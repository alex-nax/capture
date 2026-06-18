# PyInstaller entry for the bundled `captured` daemon.
#
# Normally this just launches the daemon. `--asr-selftest` is a diagnostic that
# verifies the bundled mlx runtime actually works *frozen* — it forces a Metal
# kernel (the .metallib-loading path that breaks under naive freezing) and runs a
# whisper-tiny transcription. Used by packaging to validate the self-contained app.
import sys


def _asr_selftest() -> int:
    # Activate the chosen runtime pack first (so an externally-installed engine is importable in
    # the frozen daemon — the core of the runtime-pack architecture, docs/specs/asr-runtimes.md).
    from capture_mcp.core.asr import runtimes

    rid = runtimes.activate()
    print(f"runtime activated: {rid}")

    # macOS bundles mlx; verify the Metal path. Else verify the externally-installed faster-whisper
    # pack loads its CTranslate2 C-extension in the frozen interpreter (the keystone check).
    import importlib.util

    if importlib.util.find_spec("mlx_whisper") is not None:
        import numpy as np

        import mlx.core as mx

        x = mx.ones((64, 64))
        mx.eval(x @ x)  # forces Metal kernel compile/exec from the bundled mlx.metallib
        print(f"mlx Metal OK (dtype={x.dtype})")
        import mlx_whisper

        pcm = np.zeros(16000, dtype=np.float32)
        result = mlx_whisper.transcribe(
            pcm, path_or_hf_repo="mlx-community/whisper-tiny", word_timestamps=False
        )
        print(f"mlx_whisper OK (segments={len(result.get('segments', []))})")
        return 0

    import ctranslate2  # the binary C-extension from the runtime pack

    import faster_whisper

    print(
        f"faster-whisper OK (ctranslate2={ctranslate2.__version__}, "
        f"cuda_devices={ctranslate2.get_cuda_device_count()})"
    )
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

    # Put the active ASR runtime pack on sys.path + DLL search path BEFORE anything imports an
    # engine, so the frozen daemon can use an externally-installed runtime. No-op if none chosen.
    from capture_mcp.core.asr import runtimes

    runtimes.activate()

    from capture_mcp.daemon.server import main

    main()
