"""Explicit CI acquisition only; uses Python 3.11+ standard library."""
import hashlib
import json
from pathlib import Path
import sys
import urllib.request

MANIFEST = Path(__file__).resolve().parents[2] / "src/analysis/retrieval/embedding/resources.json"


def verify(path, resource):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if path.stat().st_size != resource["size"] or digest != resource["sha256"]:
        raise ValueError(f"pinned resource size/digest mismatch: {resource['name']}")


def acquire(directory):
    manifest = json.loads(MANIFEST.read_text())
    directory.mkdir(parents=True, exist_ok=True)
    for resource in manifest["files"]:
        path = directory / resource["name"]
        if path.exists():
            verify(path, resource)
            continue
        temporary = path.with_suffix(path.suffix + ".partial")
        try:
            url = f"https://huggingface.co/{manifest['repository']}/resolve/{manifest['revision']}/{resource['name']}"
            with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as output:
                while block := response.read(65536):
                    output.write(block)
            verify(temporary, resource)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    acquire(Path(sys.argv[1]))
