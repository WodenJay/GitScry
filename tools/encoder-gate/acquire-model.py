"""Explicit CI acquisition only; no third-party Python packages required."""
import hashlib
from pathlib import Path
import sys
import urllib.request

REVISION = "8f518e882455312b086101e60691f5e6e2f05c3c"
DIGEST = "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5"
SIZE = 90_387_630


def verify(path):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if path.stat().st_size != SIZE or digest != DIGEST:
        raise ValueError("pinned FP32 model size/digest mismatch")


def acquire(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        verify(path)
        return
    temporary = path.with_suffix(".partial")
    try:
        url = f"https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/{REVISION}/model.onnx"
        with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as output:
            while block := response.read(65536):
                output.write(block)
        verify(temporary)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    acquire(Path(sys.argv[1]))
