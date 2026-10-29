"""Export a FastEnhancer 48 kHz checkpoint into the flat blob fastenhancer.c reads.

    python export_weights.py <fastenhancer repo> <checkpoint dir> <out.bin>

<checkpoint dir> is an unzipped `ckpt-v1.0.0-48khz` release asset (00500.pth +
config.yaml) from github.com/aask1357/fastenhancer. Needs torch and pyyaml.

Layout: int32[16] header, then float32 tensors in the order of `tensors()`.
Weight norm and every BatchNorm are folded into the preceding layer.
"""

import struct
import sys

import numpy as np
import torch
import yaml

MAGIC = 0x38344546  # "FE48" little-endian
VERSION = 1
HEADER_INTS = 16


def load_model(repo: str, ckpt_dir: str):
    sys.path.insert(0, repo)
    from models.fastenhancer.default.model import ONNXModel

    with open(f"{ckpt_dir}/config.yaml") as f:
        kw = yaml.safe_load(f)["model_kwargs"]
    rf = kw["rnnformer_kwargs"]
    # fastenhancer.c hardcodes everything below; a checkpoint that differs needs code changes.
    expected = dict(
        stride=4, window="hann", mask=None, activation="SiLU", resnet=False,
        weight_norm=True, stft_normalized=False, input_compression=0.3,
    )
    for k, v in expected.items():
        assert kw[k] == v, f"{k}={kw[k]}, fastenhancer.c expects {v}"
    assert kw["kernel_size"][0] == 8 and all(k == 3 for k in kw["kernel_size"][1:])
    assert kw["n_fft"] == kw["win_size"] == 2 * kw["hop_size"]
    assert not rf["attn_bias"] and not rf["post_act"] and not rf["pre_norm"]
    assert rf["positional_embedding"] == "train"

    model = ONNXModel(**kw)
    state = torch.load(f"{ckpt_dir}/00500.pth", map_location="cpu", weights_only=False)
    model.load_state_dict(state["model"])
    model.eval()
    model.remove_weight_reparameterizations()
    return model, kw


def tensors(m) -> list[torch.Tensor]:
    out = [m.enc_pre[0].weight, m.enc_pre[0].bias]
    for layer in m.encoder:
        out += [layer[0].weight, layer[0].bias]
    out += [m.rf_pre[0].weight, m.rf_pre[1].weight, m.rf_pre[1].bias]
    for i, b in enumerate(m.rf_block):
        out += [
            b.rnn.weight_ih_l0, b.rnn.weight_hh_l0, b.rnn.bias_ih_l0, b.rnn.bias_hh_l0,
            b.rnn_fc.weight, b.rnn_fc.bias,
        ]
        if i == 0:
            out.append(b.pe)
        out += [b.attn.qkv.weight, b.attn_fc.weight, b.attn_fc.bias]
    out += [m.rf_post[0].weight, m.rf_post[1].weight, m.rf_post[1].bias]
    for layer in m.decoder:
        out += [layer[0].weight, layer[0].bias, layer[2].weight, layer[2].bias]
    out += [m.dec_post[0].weight, m.dec_post[0].bias, m.dec_post[2].weight, m.dec_post[2].bias]
    return out


def main(repo: str, ckpt_dir: str, out_path: str) -> None:
    model, kw = load_model(repo, ckpt_dir)
    rf = kw["rnnformer_kwargs"]
    header = [
        MAGIC, VERSION, kw["n_fft"], kw["hop_size"], kw["channels"], len(kw["kernel_size"]) - 1,
        rf["channels"], rf["freq"], rf["num_blocks"], rf["num_heads"],
    ]
    header += [0] * (HEADER_INTS - len(header))
    with torch.no_grad():
        data = np.concatenate([t.detach().float().numpy().ravel() for t in tensors(model)])
    with open(out_path, "wb") as f:
        f.write(struct.pack(f"<{HEADER_INTS}i", *header))
        f.write(data.astype("<f4").tobytes())
    print(f"{out_path}: {data.size} floats, header {header[2:10]}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
