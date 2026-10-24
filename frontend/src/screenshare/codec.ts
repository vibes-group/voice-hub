import type { ScreenVideoCodec } from '../sfu/protocol';

type Codec = RTCRtpCodec & { mimeType: string };

export type ScreenCodecSupport = {
  send: ReadonlySet<ScreenVideoCodec>;
  receive: ReadonlySet<ScreenVideoCodec>;
};

const CODEC_PRIORITY: ScreenVideoCodec[] = ['av1', 'vp9'];

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

function supportedCodecSet(
  caps: RTCRtpCapabilities | null | undefined,
): ReadonlySet<ScreenVideoCodec> {
  const out = new Set<ScreenVideoCodec>();
  for (const codec of (caps?.codecs ?? []) as Codec[]) {
    const normalized = normalizeCodec(codec);
    if (normalized) out.add(normalized);
  }
  return out;
}

function screenCodecSupport(): ScreenCodecSupport {
  return {
    send: supportedCodecSet(globalThis.RTCRtpSender?.getCapabilities?.('video')),
    receive: supportedCodecSet(globalThis.RTCRtpReceiver?.getCapabilities?.('video')),
  };
}

export function chooseScreenCodec(
  pref: ScreenVideoCodec,
  support = screenCodecSupport(),
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
  return screenCodecSupport().receive.has(codec);
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
