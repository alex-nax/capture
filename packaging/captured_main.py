# PyInstaller entry for the bundled `captured` daemon.
#
# Normally this just launches the daemon. `--asr-selftest` is a diagnostic that
# verifies the bundled mlx runtime actually works *frozen* — it forces a Metal
# kernel (the .metallib-loading path that breaks under naive freezing) and runs a
# whisper-tiny transcription. Used by packaging to validate the self-contained app.
import sys


def _asr_selftest() -> int:
    import numpy as np

    import mlx.core as mx

    x = mx.ones((64, 64))
    mx.eval(x @ x)  # forces Metal kernel compile/exec from the bundled mlx.metallib
    print(f"mlx Metal OK (dtype={x.dtype})")

    import mlx_whisper

    pcm = np.zeros(16000, dtype=np.float32)  # 1s @ 16k; exercises the model load path
    result = mlx_whisper.transcribe(
        pcm, path_or_hf_repo="mlx-community/whisper-tiny", word_timestamps=False
    )
    print(f"mlx_whisper OK (segments={len(result.get('segments', []))})")
    return 0


if __name__ == "__main__":
    # CRITICAL for the frozen app: numba/mlx_whisper use multiprocessing, and a
    # PyInstaller child re-executes THIS entry. Without freeze_support() the child
    # falls through to main() and starts a rogue second daemon. This makes the
    # child return into multiprocessing instead of re-running the daemon.
    import multiprocessing

    multiprocessing.freeze_support()

    if "--asr-selftest" in sys.argv:
        raise SystemExit(_asr_selftest())
    from capture_mcp.daemon.server import main

    main()
