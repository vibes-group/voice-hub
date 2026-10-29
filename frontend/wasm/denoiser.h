// C ABI shared by every denoiser wasm; the AudioWorklet drives it frame by frame.
#pragma once

typedef struct Denoiser Denoiser;

// `weights` may be NULL for engines with a built-in model. NULL on failure.
Denoiser *dn_create(const void *weights, int len);
// Samples per dn_process call, mono 48 kHz.
int dn_frame_size(const Denoiser *d);
// Samples in [-1, 1]; `in` and `out` may alias. Must not allocate: the caller
// keeps a view of linear memory. Returns voice probability, or -1 without VAD.
float dn_process(Denoiser *d, const float *in, float *out);
void dn_destroy(Denoiser *d);
