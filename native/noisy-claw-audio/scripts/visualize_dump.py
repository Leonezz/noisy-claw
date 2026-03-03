#!/usr/bin/env python3
"""Visualize an audio dump directory produced by noisy-claw-audio's dump module.

Usage:
    python3 visualize_dump.py /path/to/dump_20260304_013500/

Generates a 7-row time-aligned figure:
  1. capture.pcm     — mic input (blue)
  2. speaker_out.pcm — actual speaker output at device rate (orange)
  3. render.pcm      — AEC reference as received by AEC node (cyan)
  4. aec_out.pcm     — echo-cancelled output (green)
  5. vad_pass.pcm    — audio forwarded to STT (purple)
  6. tts_out.pcm     — raw TTS chunks before resampling (red)
  7. vad_meta.csv    — speech prob, is_speech, speaking_tts, blanking

Saves visualization.png and opens an interactive window.

Dependencies: pip3 install matplotlib numpy
"""

import json
import sys
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np


def rms_envelope(samples: np.ndarray, window: int = 1024) -> np.ndarray:
    """Compute RMS envelope with a sliding window."""
    if len(samples) < window:
        return np.array([np.sqrt(np.mean(samples ** 2))])
    # Pad to make length divisible by window
    pad_len = (window - len(samples) % window) % window
    padded = np.append(samples, np.zeros(pad_len))
    reshaped = padded.reshape(-1, window)
    return np.sqrt(np.mean(reshaped ** 2, axis=1))


def plot_waveform(ax, pcm_path: Path, sample_rate: int, color: str, label: str):
    """Plot waveform + RMS envelope on given axes."""
    if not pcm_path.exists() or pcm_path.stat().st_size == 0:
        ax.text(0.5, 0.5, f"{label}: no data", transform=ax.transAxes,
                ha="center", va="center", fontsize=12, color="gray")
        ax.set_ylabel(label)
        return

    samples = np.fromfile(str(pcm_path), dtype=np.float32)
    t = np.arange(len(samples)) / sample_rate

    ax.plot(t, samples, color=color, alpha=0.4, linewidth=0.3)

    # RMS envelope
    window = 1024
    env = rms_envelope(samples, window)
    env_t = np.arange(len(env)) * window / sample_rate
    ax.plot(env_t, env, color=color, linewidth=1.5, label="RMS")
    ax.plot(env_t, -env, color=color, linewidth=1.5)

    ax.set_ylabel(label)
    ax.set_ylim(-1.0, 1.0)
    ax.legend(loc="upper right", fontsize=8)


def plot_vad_meta(ax, csv_path: Path):
    """Plot VAD metadata: speech_prob line, is_speech shading, speaking_tts background."""
    if not csv_path.exists() or csv_path.stat().st_size == 0:
        ax.text(0.5, 0.5, "vad_meta: no data", transform=ax.transAxes,
                ha="center", va="center", fontsize=12, color="gray")
        ax.set_ylabel("VAD")
        return

    # Parse CSV (skip header)
    data = []
    with open(csv_path) as f:
        header = f.readline()  # skip header
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.split(",")
            if len(parts) >= 6:
                data.append([float(x) for x in parts[:6]])

    if not data:
        ax.text(0.5, 0.5, "vad_meta: empty", transform=ax.transAxes,
                ha="center", va="center", fontsize=12, color="gray")
        ax.set_ylabel("VAD")
        return

    arr = np.array(data)
    t = arr[:, 0] / 1000.0  # ms → seconds
    speech_prob = arr[:, 1]
    is_speech = arr[:, 2].astype(bool)
    speaking_tts = arr[:, 3].astype(bool)
    blanking = arr[:, 4]
    was_speaking = arr[:, 5].astype(bool)

    # Background: speaking_tts regions
    for i in range(len(t) - 1):
        if speaking_tts[i]:
            ax.axvspan(t[i], t[i + 1], alpha=0.15, color="red", linewidth=0)

    # Shaded: is_speech regions
    for i in range(len(t) - 1):
        if is_speech[i]:
            ax.axvspan(t[i], t[i + 1], alpha=0.3, color="green", linewidth=0)

    # Blanking markers
    blanking_active = blanking > 0
    if blanking_active.any():
        ax.scatter(t[blanking_active],
                   np.full(blanking_active.sum(), 0.05),
                   marker="|", color="orange", s=30, label="blanking", zorder=3)

    # was_speaking markers
    if was_speaking.any():
        for i in range(len(t) - 1):
            if was_speaking[i]:
                ax.axvspan(t[i], t[i + 1], alpha=0.1, color="blue", linewidth=0)

    # Speech probability line
    ax.plot(t, speech_prob, color="black", linewidth=1.0, label="speech_prob")
    ax.axhline(y=0.5, color="gray", linestyle="--", linewidth=0.5, alpha=0.5)
    ax.axhline(y=0.85, color="red", linestyle="--", linewidth=0.5, alpha=0.5)

    ax.set_ylabel("VAD")
    ax.set_ylim(-0.05, 1.05)
    ax.legend(loc="upper right", fontsize=7,
              labels=["speech_prob", "threshold 0.5", "threshold 0.85",
                       "TTS playing (red bg)", "is_speech (green)",
                       "blanking", "was_speaking (blue bg)"])


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <dump_directory>")
        sys.exit(1)

    dump_dir = Path(sys.argv[1])
    if not dump_dir.is_dir():
        print(f"Error: {dump_dir} is not a directory")
        sys.exit(1)

    # Load meta.json for sample rates
    meta_path = dump_dir / "meta.json"
    if meta_path.exists():
        with open(meta_path) as f:
            meta = json.load(f)
        taps_meta = meta.get("taps", {})
    else:
        taps_meta = {}

    def get_sr(tap: str, default: int = 16000) -> int:
        return taps_meta.get(tap, {}).get("sample_rate", default)

    # Define tap layout
    taps = [
        ("capture", "Capture (mic)", "tab:blue"),
        ("speaker_out", "Speaker Out", "tab:orange"),
        ("render", "Render (AEC ref)", "tab:cyan"),
        ("aec_out", "AEC Out", "tab:green"),
        ("vad_pass", "VAD Pass", "tab:purple"),
        ("tts_out", "TTS Out", "tab:red"),
    ]

    fig, axes = plt.subplots(len(taps) + 1, 1, figsize=(18, 16), sharex=True)
    fig.suptitle(f"Audio Dump: {dump_dir.name}", fontsize=14)

    # Plot waveforms
    for i, (tap_name, label, color) in enumerate(taps):
        pcm_path = dump_dir / f"{tap_name}.pcm"
        sr = get_sr(tap_name)
        plot_waveform(axes[i], pcm_path, sr, color, label)

    # Plot VAD metadata (last row)
    vad_row = len(taps)
    plot_vad_meta(axes[vad_row], dump_dir / "vad_meta.csv")

    axes[vad_row].set_xlabel("Time (seconds)")

    plt.tight_layout()
    out_path = dump_dir / "visualization.png"
    plt.savefig(str(out_path), dpi=150)
    print(f"Saved: {out_path}")
    plt.show()


if __name__ == "__main__":
    main()
