// FastEnhancer (github.com/aask1357/fastenhancer, MIT) streaming inference,
// fp32, for the 48 kHz checkpoints. Mirrors ONNXModel + ONNXSTFT of the
// reference: one call takes `hop` new samples and returns `hop` enhanced
// samples delayed by n_fft - hop. Weights come from export_weights.py.

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "../denoiser.h"

#define MAGIC 0x38344546 // "FE48"
#define VERSION 1
#define HEADER_INTS 16
#define MAX_ENC 4
#define MAX_BLOCKS 8
#define STRIDE 4
#define K_PRE 8   // enc_pre / dec_post kernel; StridedConv1d sees K_PRE / STRIDE taps
#define K_ENC 3
#define COMPRESSION 0.3f
#define PI_F 3.14159265358979f

typedef struct {
  const float *w_ih, *w_hh, *b_ih, *b_hh, *fc_w, *fc_b, *pe, *qkv, *afc_w, *afc_b;
} Block;

struct Denoiser {
  int n_fft, hop, nbin, nf, ch, n_enc, rc, rf, n_blocks, n_heads;

  float *weights;
  const float *pre_w, *pre_b;
  const float *enc_w[MAX_ENC], *enc_b[MAX_ENC];
  float *rf_pre_lin_t; // [nf][rf], transposed so the freq projection is an axpy
  const float *rf_pre_w, *rf_pre_b;
  Block blk[MAX_BLOCKS];
  float *rf_post_lin_t; // [rf][nf]
  const float *rf_post_w, *rf_post_b;
  const float *dec_w1[MAX_ENC], *dec_b1[MAX_ENC], *dec_w2[MAX_ENC], *dec_b2[MAX_ENC];
  const float *post_w, *post_b, *up_w, *up_b;

  float *window, *window_istft, *tw_re, *tw_im;
  int *bitrev;

  float *in_cache, *out_cache, *hidden; // hidden: [n_blocks][rf][rc]

  // Scratch.
  float *fft_re, *fft_im, *spec, *mask;
  float *pre_in;               // [2*STRIDE][nf+1]
  float *enc[MAX_ENC + 1];     // [ch][nf] each
  float *x, *cat, *pad;        // [ch][nf], [2ch][nf], [ch][nf+2]
  float *freq_proj;            // [max(ch,rc)][max(rf,nf)]
  float *tok, *qkv, *attn, *gates, *tmp; // token-major RNNFormer buffers
};

// exp(x) = 2^n * e^r, |r| <= ln2/2, e^r by a degree-6 Taylor series: relative
// error ~1e-7, branch-free so the SiLU loops vectorize (libm expf does not).
static inline float fast_expf(float x) {
  x = x < -87.0f ? -87.0f : (x > 88.0f ? 88.0f : x);
  const float t = x * 1.44269504f;
  const float n = __builtin_rintf(t), f = (t - n) * 0.69314718f;
  const float p =
      1.0f + f * (1.0f + f * (0.5f + f * (1.0f / 6 + f * (1.0f / 24 + f * (1.0f / 120 + f * (1.0f / 720))))));
  union { int32_t i; float f; } s = {.i = ((int32_t)n + 127) << 23};
  return p * s.f;
}

static inline float sigmoidf(float x) { return 1.0f / (1.0f + fast_expf(-x)); }
static inline float siluf(float x) { return x / (1.0f + fast_expf(-x)); }

static void silu(float *x, int n) {
  for (int i = 0; i < n; i++) x[i] = siluf(x[i]);
}

// ---- FFT (radix-2, in place) ----

static void fft(const Denoiser *st, float *re, float *im, int inverse) {
  const int n = st->n_fft;
  for (int i = 0; i < n; i++) {
    int j = st->bitrev[i];
    if (j > i) {
      float t = re[i]; re[i] = re[j]; re[j] = t;
      t = im[i]; im[i] = im[j]; im[j] = t;
    }
  }
  for (int len = 2; len <= n; len <<= 1) {
    int half = len >> 1, step = n / len;
    for (int s = 0; s < n; s += len) {
      for (int k = 0; k < half; k++) {
        float wr = st->tw_re[k * step];
        float wi = inverse ? -st->tw_im[k * step] : st->tw_im[k * step];
        int a = s + k, b = a + half;
        float xr = re[b] * wr - im[b] * wi;
        float xi = re[b] * wi + im[b] * wr;
        re[b] = re[a] - xr; im[b] = im[a] - xi;
        re[a] += xr; im[a] += xi;
      }
    }
  }
}

// ---- Layers. Feature maps are [channels][freq], freq contiguous. ----

// 1-D convolution: out[o][t] = b[o] + sum_{i,k} w[o][i][k] * in[i][t + k], with
// `in` rows `istride` apart and already padded; cout % 4 == 0 (checked at
// create). Tiles of 4 outputs x TILE positions stay in registers across the
// whole reduction; clang vectorizes the fixed-width inner loops.
#define TILE 8
static void conv(float *out, const float *in, const float *w, const float *b,
                 int cout, int cin, int k_taps, int len, int istride) {
  for (int o = 0; o < cout; o += 4) {
    int t0 = 0;
    for (; t0 + TILE <= len; t0 += TILE) {
      float acc[4][TILE];
      for (int j = 0; j < 4; j++)
        for (int t = 0; t < TILE; t++) acc[j][t] = b[o + j];
      for (int i = 0; i < cin; i++)
        for (int k = 0; k < k_taps; k++) {
          const float *x = in + i * istride + t0 + k;
          const float w0 = w[((o + 0) * cin + i) * k_taps + k];
          const float w1 = w[((o + 1) * cin + i) * k_taps + k];
          const float w2 = w[((o + 2) * cin + i) * k_taps + k];
          const float w3 = w[((o + 3) * cin + i) * k_taps + k];
          for (int t = 0; t < TILE; t++) {
            acc[0][t] += w0 * x[t];
            acc[1][t] += w1 * x[t];
            acc[2][t] += w2 * x[t];
            acc[3][t] += w3 * x[t];
          }
        }
      for (int j = 0; j < 4; j++) memcpy(out + (o + j) * len + t0, acc[j], sizeof acc[j]);
    }
    for (; t0 < len; t0++)
      for (int j = o; j < o + 4; j++) {
        float acc = b[j];
        for (int i = 0; i < cin; i++)
          for (int k = 0; k < k_taps; k++) acc += w[(j * cin + i) * k_taps + k] * in[i * istride + t0 + k];
        out[j * len + t0] = acc;
      }
  }
}

static void conv1x1(float *out, const float *in, const float *w, const float *b,
                    int cout, int cin, int len) {
  conv(out, in, w, b, cout, cin, 1, len, len);
}

// Kernel 3, padding 1, channel count preserved; `pad` is [c][len+2] scratch.
static void conv3(float *out, const float *in, const float *w, const float *b,
                  int c, int len, float *pad) {
  const int pl = len + 2;
  for (int i = 0; i < c; i++) {
    pad[i * pl] = 0.0f;
    memcpy(pad + i * pl + 1, in + i * len, len * sizeof(float));
    pad[i * pl + len + 1] = 0.0f;
  }
  conv(out, pad, w, b, c, c, K_ENC, len, pl);
}

// out[c][g] = sum_f lin_t[f][g] * in[c][f]  (nn.Linear over the freq axis)
static void freq_linear(float *out, const float *in, const float *lin_t,
                        int c, int fin, int fout) {
  memset(out, 0, (size_t)c * fout * sizeof(float));
  for (int ci = 0; ci < c; ci++) {
    float *y = out + ci * fout;
    const float *x = in + ci * fin;
    for (int f = 0; f < fin; f++) {
      const float xv = x[f];
      const float *l = lin_t + f * fout;
      for (int g = 0; g < fout; g++) y[g] += xv * l[g];
    }
  }
}

// y[r] = b[r] + sum_c w[r][c] * x[c]   (b may be NULL)
static void matvec(float *y, const float *w, const float *x, const float *b, int rows, int cols) {
  for (int r = 0; r < rows; r++) {
    const float *wr = w + r * cols;
    float acc = b ? b[r] : 0.0f;
    for (int c = 0; c < cols; c++) acc += wr[c] * x[c];
    y[r] = acc;
  }
}

static void rnnformer_block(Denoiser *st, const Block *bk, float *h) {
  const int rc = st->rc, rf = st->rf, nh = st->n_heads, d = rc / nh;
  float *gi = st->gates, *gh = st->gates + 3 * rc, *tmp = st->tmp;

  // GRU along time, one independent sequence per frequency token.
  for (int f = 0; f < rf; f++) {
    float *x = st->tok + f * rc, *hf = h + f * rc;
    matvec(gi, bk->w_ih, x, bk->b_ih, 3 * rc, rc);
    matvec(gh, bk->w_hh, hf, bk->b_hh, 3 * rc, rc);
    for (int i = 0; i < rc; i++) {
      float r = sigmoidf(gi[i] + gh[i]);
      float z = sigmoidf(gi[rc + i] + gh[rc + i]);
      float n = tanhf(gi[2 * rc + i] + r * gh[2 * rc + i]);
      hf[i] = (1.0f - z) * n + z * hf[i];
    }
    matvec(tmp, bk->fc_w, hf, bk->fc_b, rc, rc);
    for (int i = 0; i < rc; i++) x[i] += tmp[i];
    if (bk->pe)
      for (int i = 0; i < rc; i++) x[i] += bk->pe[f * rc + i];
  }

  // Multi-head self-attention across frequency tokens.
  for (int f = 0; f < rf; f++) matvec(st->qkv + f * 3 * rc, bk->qkv, st->tok + f * rc, NULL, 3 * rc, rc);
  const float scale = 1.0f / sqrtf((float)d);
  float *scores = st->gates; // rf <= 6 * rc is checked at create
  for (int hd = 0; hd < nh; hd++) {
    const int off = hd * 3 * d;
    for (int f = 0; f < rf; f++) {
      const float *q = st->qkv + f * 3 * rc + off;
      float mx = -INFINITY;
      for (int g = 0; g < rf; g++) {
        const float *k = st->qkv + g * 3 * rc + off + d;
        float s = 0.0f;
        for (int i = 0; i < d; i++) s += q[i] * k[i];
        scores[g] = s * scale;
        if (scores[g] > mx) mx = scores[g];
      }
      float sum = 0.0f;
      for (int g = 0; g < rf; g++) sum += (scores[g] = expf(scores[g] - mx));
      float *o = st->attn + f * rc + hd * d;
      for (int i = 0; i < d; i++) o[i] = 0.0f;
      for (int g = 0; g < rf; g++) {
        const float p = scores[g] / sum, *v = st->qkv + g * 3 * rc + off + 2 * d;
        for (int i = 0; i < d; i++) o[i] += p * v[i];
      }
    }
  }
  for (int f = 0; f < rf; f++) {
    matvec(tmp, bk->afc_w, st->attn + f * rc, bk->afc_b, rc, rc);
    float *x = st->tok + f * rc;
    for (int i = 0; i < rc; i++) x[i] += tmp[i];
  }
}

// spec: [2][nbin] compressed spectrum (real, imag). Writes the complex mask.
static void network(Denoiser *st) {
  const int ch = st->ch, nf = st->nf, nbin = st->nbin, rc = st->rc, rf = st->rf;
  const int kp = K_PRE / STRIDE, cin_pre = 2 * STRIDE, lp = nf + kp - 1;

  // Encoder PreNet: StridedConv1d == pad, fold stride into channels, conv.
  const int pad = (K_PRE - STRIDE) / 2;
  for (int j = 0; j < STRIDE; j++)
    for (int c = 0; c < 2; c++)
      for (int u = 0; u < lp; u++) {
        int p = u * STRIDE + j - pad;
        st->pre_in[(j * 2 + c) * lp + u] = (p >= 0 && p < nbin) ? st->spec[c * nbin + p] : 0.0f;
      }
  conv(st->enc[0], st->pre_in, st->pre_w, st->pre_b, ch, cin_pre, kp, nf, lp);
  silu(st->enc[0], ch * nf);

  for (int l = 0; l < st->n_enc; l++) {
    conv3(st->enc[l + 1], st->enc[l], st->enc_w[l], st->enc_b[l], ch, nf, st->pad);
    silu(st->enc[l + 1], ch * nf);
  }

  // RNNFormer PreNet, then to token-major [rf][rc].
  freq_linear(st->freq_proj, st->enc[st->n_enc], st->rf_pre_lin_t, ch, nf, rf);
  conv1x1(st->x, st->freq_proj, st->rf_pre_w, st->rf_pre_b, rc, ch, rf);
  for (int c = 0; c < rc; c++)
    for (int f = 0; f < rf; f++) st->tok[f * rc + c] = st->x[c * rf + f];

  for (int b = 0; b < st->n_blocks; b++)
    rnnformer_block(st, &st->blk[b], st->hidden + b * rf * rc);

  // RNNFormer PostNet.
  for (int c = 0; c < rc; c++)
    for (int f = 0; f < rf; f++) st->x[c * rf + f] = st->tok[f * rc + c];
  freq_linear(st->freq_proj, st->x, st->rf_post_lin_t, rc, rf, nf);
  conv1x1(st->x, st->freq_proj, st->rf_post_w, st->rf_post_b, ch, rc, nf);

  // Decoder: concat with the matching encoder output (deepest first).
  for (int l = 0; l < st->n_enc; l++) {
    memcpy(st->cat, st->x, (size_t)ch * nf * sizeof(float));
    memcpy(st->cat + ch * nf, st->enc[st->n_enc - l], (size_t)ch * nf * sizeof(float));
    conv1x1(st->x, st->cat, st->dec_w1[l], st->dec_b1[l], ch, 2 * ch, nf);
    silu(st->x, ch * nf);
    conv3(st->x, st->x, st->dec_w2[l], st->dec_b2[l], ch, nf, st->pad);
    silu(st->x, ch * nf);
  }

  // Decoder PostNet: 1x1 conv, then ConvTranspose1d back to nbin bins.
  memcpy(st->cat, st->x, (size_t)ch * nf * sizeof(float));
  memcpy(st->cat + ch * nf, st->enc[0], (size_t)ch * nf * sizeof(float));
  conv1x1(st->x, st->cat, st->post_w, st->post_b, ch, 2 * ch, nf);
  silu(st->x, ch * nf);
  for (int o = 0; o < 2; o++)
    for (int p = 0; p < nbin; p++) st->mask[o * nbin + p] = st->up_b[o];
  for (int i = 0; i < ch; i++)
    for (int o = 0; o < 2; o++) {
      const float *w = st->up_w + (i * 2 + o) * K_PRE;
      float *m = st->mask + o * nbin;
      const float *x = st->x + i * nf;
      for (int t = 0; t < nf; t++)
        for (int k = 0; k < K_PRE; k++) {
          int p = t * STRIDE + k - pad;
          if (p >= 0 && p < nbin) m[p] += w[k] * x[t];
        }
    }
}

int dn_frame_size(const Denoiser *st) { return st->hop; }

float dn_process(Denoiser *st, const float *in, float *out) {
  const int n = st->n_fft, hop = st->hop, nbin = st->nbin, keep = n - hop;
  float *re = st->fft_re, *im = st->fft_im;

  for (int i = 0; i < keep; i++) re[i] = st->in_cache[i] * st->window[i];
  for (int i = 0; i < hop; i++) re[keep + i] = in[i] * st->window[keep + i];
  memmove(st->in_cache, st->in_cache + hop, (keep - hop) * sizeof(float));
  memcpy(st->in_cache + keep - hop, in, hop * sizeof(float));
  memset(im, 0, n * sizeof(float));
  fft(st, re, im, 0);

  // Power-law compression; the Nyquist bin is dropped.
  for (int k = 0; k < nbin; k++) {
    float mag = sqrtf(re[k] * re[k] + im[k] * im[k]);
    float g = powf(mag < 1e-5f ? 1e-5f : mag, COMPRESSION - 1.0f);
    st->spec[k] = re[k] * g;
    st->spec[nbin + k] = im[k] * g;
  }

  network(st);

  // Complex mask, decompression, Hermitian spectrum for the inverse FFT.
  for (int k = 0; k < nbin; k++) {
    float xr = st->spec[k], xi = st->spec[nbin + k];
    float mr = st->mask[k], mi = st->mask[nbin + k];
    float yr = xr * mr - xi * mi, yi = xr * mi + xi * mr;
    float g = powf(sqrtf(yr * yr + yi * yi), 1.0f / COMPRESSION - 1.0f);
    re[k] = yr * g;
    im[k] = yi * g;
  }
  re[nbin] = im[nbin] = 0.0f;
  im[0] = 0.0f;
  for (int k = 1; k < nbin; k++) {
    re[n - k] = re[k];
    im[n - k] = -im[k];
  }
  fft(st, re, im, 1);

  const float inv_n = 1.0f / (float)n;
  for (int i = 0; i < n; i++) re[i] *= inv_n * st->window_istft[i];
  for (int i = 0; i < keep; i++) re[i] += st->out_cache[i];
  memcpy(out, re, hop * sizeof(float));
  memcpy(st->out_cache, re + hop, keep * sizeof(float));
  return -1.0f; // no VAD
}

// ---- Setup ----

static float *zalloc(size_t n) { return calloc(n ? n : 1, sizeof(float)); }

Denoiser *dn_create(const void *blob, int len) {
  if (!blob || len < HEADER_INTS * 4) return NULL;
  const int32_t *hdr = blob;
  if (hdr[0] != MAGIC || hdr[1] != VERSION) return NULL;

  Denoiser *st = calloc(1, sizeof(Denoiser));
  if (!st) return NULL;
  st->n_fft = hdr[2];
  st->hop = hdr[3];
  st->ch = hdr[4];
  st->n_enc = hdr[5];
  st->rc = hdr[6];
  st->rf = hdr[7];
  st->n_blocks = hdr[8];
  st->n_heads = hdr[9];
  st->nbin = st->n_fft / 2;
  st->nf = st->nbin / STRIDE;
  const int n = st->n_fft, ch = st->ch, nf = st->nf, rc = st->rc, rf = st->rf;
  if (n < 8 * STRIDE || (n & (n - 1)) || st->hop * 2 != n || st->n_enc < 0 || st->n_enc > MAX_ENC ||
      st->n_blocks < 1 || st->n_blocks > MAX_BLOCKS || st->n_heads < 1 || ch < 4 || rc < 4 || rf < 1 ||
      rc % st->n_heads || ch % 4 || rc % 4 || rf > 6 * rc) {
    free(st);
    return NULL;
  }

  // Walk the blob in export_weights.py order.
  const int nfloats = (len - HEADER_INTS * 4) / 4;
  st->weights = malloc((size_t)nfloats * sizeof(float));
  if (!st->weights) { free(st); return NULL; }
  memcpy(st->weights, (const char *)blob + HEADER_INTS * 4, (size_t)nfloats * sizeof(float));
  const float *p = st->weights;
#define TAKE(n_) (p += (n_), p - (n_))
  st->pre_w = TAKE(ch * 2 * STRIDE * (K_PRE / STRIDE));
  st->pre_b = TAKE(ch);
  for (int l = 0; l < st->n_enc; l++) {
    st->enc_w[l] = TAKE(ch * ch * K_ENC);
    st->enc_b[l] = TAKE(ch);
  }
  const float *rf_pre_lin = TAKE(rf * nf);
  st->rf_pre_w = TAKE(rc * ch);
  st->rf_pre_b = TAKE(rc);
  for (int b = 0; b < st->n_blocks; b++) {
    Block *bk = &st->blk[b];
    bk->w_ih = TAKE(3 * rc * rc);
    bk->w_hh = TAKE(3 * rc * rc);
    bk->b_ih = TAKE(3 * rc);
    bk->b_hh = TAKE(3 * rc);
    bk->fc_w = TAKE(rc * rc);
    bk->fc_b = TAKE(rc);
    bk->pe = b == 0 ? TAKE(rf * rc) : NULL;
    bk->qkv = TAKE(3 * rc * rc);
    bk->afc_w = TAKE(rc * rc);
    bk->afc_b = TAKE(rc);
  }
  const float *rf_post_lin = TAKE(nf * rf);
  st->rf_post_w = TAKE(ch * rc);
  st->rf_post_b = TAKE(ch);
  for (int l = 0; l < st->n_enc; l++) {
    st->dec_w1[l] = TAKE(ch * 2 * ch);
    st->dec_b1[l] = TAKE(ch);
    st->dec_w2[l] = TAKE(ch * ch * K_ENC);
    st->dec_b2[l] = TAKE(ch);
  }
  st->post_w = TAKE(ch * 2 * ch);
  st->post_b = TAKE(ch);
  st->up_w = TAKE(ch * 2 * K_PRE);
  st->up_b = TAKE(2);
#undef TAKE
  if (p - st->weights != nfloats) {
    dn_destroy(st);
    return NULL;
  }

  st->rf_pre_lin_t = zalloc((size_t)nf * rf);
  st->rf_post_lin_t = zalloc((size_t)rf * nf);
  for (int g = 0; g < rf; g++)
    for (int f = 0; f < nf; f++) {
      st->rf_pre_lin_t[f * rf + g] = rf_pre_lin[g * nf + f];
      st->rf_post_lin_t[g * nf + f] = rf_post_lin[f * rf + g];
    }

  // Periodic Hann (torch.hann_window); the synthesis window divides by the
  // overlap-added squared window so analysis+synthesis is an identity.
  st->window = zalloc(n);
  st->window_istft = zalloc(n);
  for (int i = 0; i < n; i++) st->window[i] = 0.5f - 0.5f * cosf(2.0f * PI_F * i / n);
  for (int i = 0; i < n; i++) {
    float a = st->window[i], b = st->window[(i + st->hop) % n];
    st->window_istft[i] = a / (a * a + b * b);
  }
  st->tw_re = zalloc(n / 2);
  st->tw_im = zalloc(n / 2);
  for (int k = 0; k < n / 2; k++) {
    st->tw_re[k] = cosf(2.0f * PI_F * k / n);
    st->tw_im[k] = -sinf(2.0f * PI_F * k / n);
  }
  st->bitrev = calloc(n, sizeof(int));
  int bits = 0;
  while ((1 << bits) < n) bits++;
  for (int i = 0; i < n; i++) {
    int r = 0;
    for (int b = 0; b < bits; b++) r |= ((i >> b) & 1) << (bits - 1 - b);
    st->bitrev[i] = r;
  }

  const int keep = n - st->hop, wide = ch > rc ? ch : rc, flen = nf > rf ? nf : rf;
  st->in_cache = zalloc(keep);
  st->out_cache = zalloc(keep);
  st->hidden = zalloc((size_t)st->n_blocks * rf * rc);
  st->fft_re = zalloc(n);
  st->fft_im = zalloc(n);
  st->spec = zalloc(2 * st->nbin);
  st->mask = zalloc(2 * st->nbin);
  st->pre_in = zalloc(2 * STRIDE * (nf + 1));
  for (int l = 0; l <= st->n_enc; l++) st->enc[l] = zalloc((size_t)ch * nf);
  st->x = zalloc((size_t)wide * flen);
  st->cat = zalloc((size_t)2 * ch * nf);
  st->pad = zalloc((size_t)ch * (nf + 2));
  st->freq_proj = zalloc((size_t)wide * flen);
  st->tok = zalloc((size_t)rf * rc);
  st->qkv = zalloc((size_t)rf * 3 * rc);
  st->attn = zalloc((size_t)rf * rc);
  st->gates = zalloc((size_t)6 * rc);
  st->tmp = zalloc(rc);
  return st;
}

void dn_destroy(Denoiser *st) {
  if (!st) return;
  float *bufs[] = {
      st->weights, st->rf_pre_lin_t, st->rf_post_lin_t, st->window, st->window_istft,
      st->tw_re, st->tw_im, st->in_cache, st->out_cache, st->hidden, st->fft_re, st->fft_im,
      st->spec, st->mask, st->pre_in, st->x, st->cat, st->pad, st->freq_proj, st->tok, st->qkv,
      st->attn, st->gates, st->tmp,
  };
  for (size_t i = 0; i < sizeof bufs / sizeof *bufs; i++) free(bufs[i]);
  for (int l = 0; l <= MAX_ENC; l++) free(st->enc[l]);
  free(st->bitrev);
  free(st);
}
