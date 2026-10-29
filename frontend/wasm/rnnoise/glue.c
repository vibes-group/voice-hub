// xiph/rnnoise behind the denoiser.h ABI. RNNoise expects int16-range floats.

#include <stdlib.h>

#include "../denoiser.h"
#include "rnnoise.h"

#define FRAME 480

struct Denoiser {
  DenoiseState *st;
  float buf[FRAME];
};

Denoiser *dn_create(const void *weights, int len) {
  (void)weights;
  (void)len;
  if (rnnoise_get_frame_size() != FRAME) return NULL;
  Denoiser *d = malloc(sizeof(Denoiser));
  if (!d) return NULL;
  d->st = rnnoise_create(NULL);
  if (!d->st) {
    free(d);
    return NULL;
  }
  return d;
}

int dn_frame_size(const Denoiser *d) {
  (void)d;
  return FRAME;
}

float dn_process(Denoiser *d, const float *in, float *out) {
  for (int i = 0; i < FRAME; i++) d->buf[i] = in[i] * 32768.0f;
  float vad = rnnoise_process_frame(d->st, d->buf, d->buf);
  for (int i = 0; i < FRAME; i++) out[i] = d->buf[i] * (1.0f / 32768.0f);
  return vad;
}

void dn_destroy(Denoiser *d) {
  if (!d) return;
  rnnoise_destroy(d->st);
  free(d);
}
