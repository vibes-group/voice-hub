#!/usr/bin/env bash
# Rebuilds the denoiser wasm modules into src/audio/wasm/ (committed).
# Needs docker, git, curl. FastEnhancer weights come from
# fastenhancer/export_weights.py, not from here.
set -euo pipefail
cd "$(dirname "$0")"

IMAGE=emscripten/emsdk:6.0.13@sha256:fb7864e398acfda863dfcea78b8bc8adad359fa60c6a0bbeb845931de64aa68a
RNNOISE_COMMIT=70f1d256acd4b34a572f999a05c87bf00b67730d
OUT=../src/audio/wasm
CACHE=.cache

mkdir -p "$CACHE" "$OUT"
[ -d "$CACHE/rnnoise/.git" ] || git clone -q https://github.com/xiph/rnnoise "$CACHE/rnnoise"
git -C "$CACHE/rnnoise" checkout -q "$RNNOISE_COMMIT"

# The model is not in git; model_version pins its sha256.
model_hash=$(cat "$CACHE/rnnoise/model_version")
model="$CACHE/rnnoise_data-$model_hash.tar.gz"
[ -f "$model" ] || curl -sSfL -o "$model" "https://media.xiph.org/rnnoise/models/rnnoise_data-$model_hash.tar.gz"
echo "$model_hash  $model" | sha256sum -c --quiet
tar xzf "$model" -C "$CACHE/rnnoise" src/rnnoise_data.c src/rnnoise_data.h

LINK="-sSTANDALONE_WASM --no-entry -sFILESYSTEM=0 -sMALLOC=emmalloc -sALLOW_MEMORY_GROWTH=1 \
  -sSTACK_SIZE=256KB -sEXPORTED_FUNCTIONS=_dn_create,_dn_frame_size,_dn_process,_dn_destroy,_malloc,_free"

# RNNoise: -mavx selects vec_avx.h, whose intrinsics emscripten lowers to wasm
# SIMD; ~4x faster than the generic C path that -msimd128 alone leaves scalar.
RNNOISE_SRC="denoise.c rnn.c pitch.c kiss_fft.c celt_lpc.c nnet.c nnet_default.c \
  parse_lpcnet_weights.c rnnoise_data.c rnnoise_tables.c"

# FastEnhancer: reassociation lets the reductions vectorize; outputs stay
# within float rounding of the reference ONNX graph.
FE_FLAGS="-fno-math-errno -fassociative-math -fno-signed-zeros -fno-trapping-math"

docker run --rm -u "$(id -u):$(id -g)" -e EM_CACHE=/src/wasm/$CACHE/em -v "$PWD/..:/src" -w /src/wasm "$IMAGE" bash -euc "
  cd $CACHE/rnnoise/src
  emcc -O3 -msimd128 -mavx -DDISABLE_DEBUG_FLOAT -I../include -I. $RNNOISE_SRC \
    /src/wasm/rnnoise/glue.c $LINK -o /src/wasm/$OUT/rnnoise.wasm
  cd /src/wasm
  emcc -O3 -msimd128 -Wall -Wextra $FE_FLAGS fastenhancer/fastenhancer.c $LINK \
    -o $OUT/fastenhancer.wasm
"
chmod a-x "$OUT"/*.wasm
ls -l "$OUT"
