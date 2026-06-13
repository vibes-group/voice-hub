import { buildScreenParams, type ScreenCodecPref } from './params';
// ScreenVideoCodec lives in the protocol mirror (Go is the source of truth).
import type { ScreenVideoCodec } from '../sfu/protocol';

type Codec = RTCRtpCodec & { mimeType: string };

export type ScreenCodecSupport = {
  send: ReadonlySet<ScreenVideoCodec>;
  receive: ReadonlySet<ScreenVideoCodec>;
  av1HardwareLikely: boolean | null;
  vp9HardwareLikely: boolean | null;
};

const CODEC_PRIORITY: ScreenVideoCodec[] = ['av1', 'vp9'];
const WEB_CODECS: Record<ScreenVideoCodec, string> = {
  av1: 'av01.0.13M.08',
  vp9: 'vp09.00.51.08',
};

let supportProbe: Promise<ScreenCodecSupport> | null = null;
let lastSupport: ScreenCodecSupport | null = null;

export function isScreenVideoCodec(value: unknown): value is ScreenVideoCodec {
  return value === 'av1' || value === 'vp9';
}

function normalizeCodec(codec: Codec): ScreenVideoCodec | null {
  switch (codec.mimeType.split('/')[1]?.toUpperCase()) {
    case 'AV1':
      return 'av1';
    case 'VP9':
      return 'vp9';
    default:
      return null;
  }
}

function supportedCodecSet(codecs: readonly Codec[] | undefined): ReadonlySet<ScreenVideoCodec> {
  const out = new Set<ScreenVideoCodec>();
  for (const codec of codecs ?? []) {
    const normalized = normalizeCodec(codec);
    if (normalized) out.add(normalized);
  }
  return out;
}

function senderVideoCapabilities(): RTCRtpCapabilities | null {
  return globalThis.RTCRtpSender?.getCapabilities?.('video') ?? null;
}

function receiverVideoCapabilities(): RTCRtpCapabilities | null {
  return globalThis.RTCRtpReceiver?.getCapabilities?.('video') ?? null;
}

async function probeHardware(codec: ScreenVideoCodec): Promise<boolean | null> {
  const videoEncoder = globalThis.VideoEncoder;
  if (!videoEncoder?.isConfigSupported) return null;
  try {
    // Probe at the ceiling (1440p60) so a "yes" means HW can carry any user
    // pick. False/null at the ceiling doesn't rule out lower resolutions.
    const ceiling = buildScreenParams('1440', 60);
    const result = await videoEncoder.isConfigSupported({
      codec: WEB_CODECS[codec],
      hardwareAcceleration: 'prefer-hardware',
      width: ceiling.width ?? 2560,
      height: ceiling.height ?? 1440,
      framerate: ceiling.fps,
      bitrate: ceiling.maxBitrate,
    });
    return result.supported ?? false;
  } catch {
    return null;
  }
}

export function primeScreenCodecProfile(): Promise<ScreenCodecSupport> {
  if (supportProbe) return supportProbe;
  supportProbe = (async () => {
    const senderCaps = senderVideoCapabilities();
    const receiverCaps = receiverVideoCapabilities();
    const [av1HardwareLikely, vp9HardwareLikely] = await Promise.all([
      probeHardware('av1'),
      probeHardware('vp9'),
    ]);
    lastSupport = {
      send: supportedCodecSet(senderCaps?.codecs as Codec[] | undefined),
      receive: supportedCodecSet(receiverCaps?.codecs as Codec[] | undefined),
      av1HardwareLikely,
      vp9HardwareLikely,
    };
    return lastSupport;
  })();
  return supportProbe;
}

function getScreenCodecSupportSync(): ScreenCodecSupport {
  if (lastSupport) return lastSupport;
  const senderCaps = senderVideoCapabilities();
  const receiverCaps = receiverVideoCapabilities();
  return {
    send: supportedCodecSet(senderCaps?.codecs as Codec[] | undefined),
    receive: supportedCodecSet(receiverCaps?.codecs as Codec[] | undefined),
    av1HardwareLikely: null,
    vp9HardwareLikely: null,
  };
}

export function chooseScreenCodec(
  pref: ScreenCodecPref,
  support = getScreenCodecSupportSync(),
): ScreenVideoCodec | null {
  if (support.send.has(pref)) return pref;
  // Pref unsupported on this device — fall back to whichever screen codec the
  // device does support, preserving the AV1 > VP9 order.
  for (const codec of CODEC_PRIORITY) {
    if (support.send.has(codec)) return codec;
  }
  return null;
}

export function canReceiveScreenCodec(codec: ScreenVideoCodec): boolean {
  return getScreenCodecSupportSync().receive.has(codec);
}

export function orderScreenCodecs<T extends Codec>(
  codecs: readonly T[],
  preferred: ScreenVideoCodec,
): T[] {
  return [...codecs].sort((a, b) => {
    const ac = normalizeCodec(a);
    const bc = normalizeCodec(b);
    const score = (codec: ScreenVideoCodec | null): number => {
      if (codec === preferred) return 0;
      const idx = codec ? CODEC_PRIORITY.indexOf(codec) : -1;
      return idx >= 0 ? idx + 1 : 99;
    };
    return score(ac) - score(bc);
  });
}

export function applyScreenCodecPreferences(
  transceiver: RTCRtpTransceiver,
  caps: RTCRtpCapabilities,
  codec: ScreenVideoCodec,
): void {
  const ordered = orderScreenCodecs(caps.codecs as Codec[], codec).filter((c) => {
    const normalized = normalizeCodec(c);
    return codec === 'vp9' ? normalized === 'vp9' : normalized !== null;
  });
  if (ordered.length > 0) transceiver.setCodecPreferences(ordered);
}
