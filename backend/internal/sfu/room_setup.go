package sfu

import (
	"time"

	"github.com/pion/interceptor"
	"github.com/pion/interceptor/pkg/cc"
	"github.com/pion/interceptor/pkg/gcc"
	"github.com/pion/interceptor/pkg/intervalpli"
	"github.com/pion/interceptor/pkg/nack"
	"github.com/pion/interceptor/pkg/twcc"
	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/dd"
)

// NewRoom creates and configures a Room with audio codecs, screen-share video
// codecs, and the full interceptor chain (RTCP reports, interval PLI, NACK,
// GCC bandwidth estimator, TWCC header extension + sender).
func NewRoom(cfg Config) (*Room, error) {
	settingEngine := webrtc.SettingEngine{}
	if len(cfg.NAT1To1IPs) > 0 {
		settingEngine.SetICEAddressRewriteRules(webrtc.ICEAddressRewriteRule{
			External:        cfg.NAT1To1IPs,
			AsCandidateType: webrtc.ICECandidateTypeHost,
		})
	}
	if cfg.UDPPortMin > 0 && cfg.UDPPortMax >= cfg.UDPPortMin {
		if err := settingEngine.SetEphemeralUDPPortRange(cfg.UDPPortMin, cfg.UDPPortMax); err != nil {
			return nil, err
		}
	}
	mediaEngine := &webrtc.MediaEngine{}
	if err := mediaEngine.RegisterCodec(webrtc.RTPCodecParameters{
		RTPCodecCapability: webrtc.RTPCodecCapability{
			MimeType:    webrtc.MimeTypeOpus,
			ClockRate:   48000,
			Channels:    2,
			SDPFmtpLine: "minptime=10;useinbandfec=1;usedtx=1;stereo=0",
		},
		PayloadType: 111,
	}, webrtc.RTPCodecTypeAudio); err != nil {
		return nil, err
	}
	// Screen-share video codecs. AV1 stays preferred by client-side codec
	// ordering; VP9 is registered as the compatibility fallback when AV1 is
	// absent or has proven CPU-bound on this client.
	if err := mediaEngine.RegisterCodec(webrtc.RTPCodecParameters{
		RTPCodecCapability: webrtc.RTPCodecCapability{
			MimeType:    webrtc.MimeTypeAV1,
			ClockRate:   90000,
			SDPFmtpLine: "level-idx=5;profile=0;tier=0",
		},
		PayloadType: 45,
	}, webrtc.RTPCodecTypeVideo); err != nil {
		return nil, err
	}
	if err := mediaEngine.RegisterCodec(webrtc.RTPCodecParameters{
		RTPCodecCapability: webrtc.RTPCodecCapability{
			MimeType:  webrtc.MimeTypeVP9,
			ClockRate: 90000,
		},
		PayloadType: 98,
	}, webrtc.RTPCodecTypeVideo); err != nil {
		return nil, err
	}
	// DD extension negotiated here so per-PC extension IDs are included in SDP.
	if err := mediaEngine.RegisterHeaderExtension(
		webrtc.RTPHeaderExtensionCapability{URI: dd.RTPExtensionURI},
		webrtc.RTPCodecTypeVideo,
	); err != nil {
		return nil, err
	}

	// Stats interceptor skipped — getStats is never consumed server-side.
	ir := &interceptor.Registry{}
	if err := webrtc.ConfigureRTCPReports(ir); err != nil {
		return nil, err
	}
	pliFactory, err := intervalpli.NewReceiverInterceptor(
		intervalpli.GeneratorInterval(3 * time.Second),
	)
	if err != nil {
		return nil, err
	}
	ir.Add(pliFactory)
	nackFactory, err := nack.NewResponderInterceptor()
	if err != nil {
		return nil, err
	}
	ir.Add(nackFactory)

	ccFactory, err := cc.NewInterceptor(func() (cc.BandwidthEstimator, error) {
		// NoOpPacer: we only want gcc's BWE estimate (for bwCapTID); the
		// default LeakyBucketPacer queues packets at the estimated rate and
		// stalls the wire when the encoder briefly outpaces it.
		return gcc.NewSendSideBWE(
			gcc.SendSideBWEInitialBitrate(bweInitialBitrate),
			gcc.SendSideBWEPacer(gcc.NewNoOpPacer()),
		)
	})
	if err != nil {
		return nil, err
	}

	r := &Room{
		peers:                 make(map[string]*peer),
		tracks:                make(map[string]*webrtc.TrackLocalStaticRTP),
		screenSessionsByToken: make(map[string]*ScreenShareSession),
		cfg:                   cfg,
	}
	ccFactory.OnNewPeerConnection(func(_ string, bwe cc.BandwidthEstimator) {
		r.pendingBWE = bwe
	})

	// Interceptor chain order matters: last-added is OUTERMOST on the write
	// path. cc/gcc's OnSent reads the TWCC header extension, so the writer
	// that SETS the extension must wrap cc (be outer).
	//
	// Two distinct TWCC interceptors:
	//   - HeaderExtensionInterceptor sets the seq# in outgoing RTP headers.
	//   - SenderInterceptor generates RTCP feedback for INCOMING RTP and
	//     does not touch the RTP write path.
	// We need both: HE outboard of cc so cc.OnSent finds the extension,
	// and Sender so publishers receive TWCC feedback for their own BWE.
	ir.Add(ccFactory)
	twccHeaderExt, err := twcc.NewHeaderExtensionInterceptor()
	if err != nil {
		return nil, err
	}
	ir.Add(twccHeaderExt)
	twccSender, err := twcc.NewSenderInterceptor()
	if err != nil {
		return nil, err
	}
	ir.Add(twccSender)

	r.api = webrtc.NewAPI(
		webrtc.WithSettingEngine(settingEngine),
		webrtc.WithMediaEngine(mediaEngine),
		webrtc.WithInterceptorRegistry(ir),
	)
	return r, nil
}
