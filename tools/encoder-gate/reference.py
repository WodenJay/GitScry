"""Reproduce #109's MiniLM protocol with an independent Python CPU reference.

No download/install here. Give a verified local resource directory and an output
file. Uses existing packages; if missing, install with uv in this project's .venv.
Reference: #109 experiment embed_corpus.py:57-117 (offset-prefix, not ID decode).
"""
import hashlib
import json
from pathlib import Path
import sys

import numpy as np
import onnxruntime as ort
import tokenizers
from tokenizers import Tokenizer

MODULE = Path(__file__).resolve().parents[2] / "src/analysis/retrieval/embedding"


def verify(directory):
    manifest = json.loads((MODULE / "resources.json").read_text())
    for entry in manifest["files"]:
        path = directory / entry["name"]
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if path.stat().st_size != entry["size"] or digest != entry["sha256"]:
            raise ValueError(f"Wrong pinned resource: {path}")


def generate(directory):
    verify(directory)
    tokenizer = Tokenizer.from_file(str(directory / "tokenizer.json"))
    tokenizer.no_truncation()
    tokenizer.no_padding()
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    session = ort.InferenceSession(str(directory / "model.onnx"), sess_options=options,
                                   providers=["CPUExecutionProvider"])

    def prefix(text, budget):
        n = min(len(text), max(4096, budget * 16))
        while True:
            encoded = tokenizer.encode(text[:n], add_special_tokens=False)
            if len(encoded.ids) > budget:
                return text[:encoded.offsets[budget - 1][1]] if budget else ""
            if n == len(text):
                return text
            n = min(len(text), n * 2)

    def document(message, paths):
        title, _, body = message.decode("utf-8", errors="replace").partition("\n")
        title = prefix(title.rstrip("\r"), 64)
        paths = prefix("\n".join(p.decode("utf-8", errors="replace") for p in paths), 32)
        def build(body):
            return f"Title: {title}\nBody: {body}\nPaths: {paths}"
        body = prefix(body, 256 - len(tokenizer.encode(build("")).ids))
        encoded = tokenizer.encode(build(body))
        while len(encoded.ids) > 256:
            tokens = tokenizer.encode(body, add_special_tokens=False)
            body = body[:tokens.offsets[-2][1]] if len(tokens.ids) > 1 else ""
            encoded = tokenizer.encode(build(body))
        assert len(encoded.ids) <= 256
        return build(body), encoded.ids, encoded.type_ids

    def embed(inputs):
        width = max(len(ids) for ids, _ in inputs)
        ids = np.zeros((len(inputs), width), dtype=np.int64)
        mask = np.zeros_like(ids)
        types = np.zeros_like(ids)
        for i, (tokens, token_types) in enumerate(inputs):
            ids[i, :len(tokens)] = tokens
            types[i, :len(tokens)] = token_types
            mask[i, :len(tokens)] = 1
        output = session.run(None, dict(input_ids=ids, attention_mask=mask, token_type_ids=types))[0]
        vectors = ((output * mask[:, :, None]).sum(axis=1) / mask.sum(axis=1)[:, None]).astype(np.float32)
        vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
        assert vectors.shape == (len(inputs), 384) and np.isfinite(vectors).all()
        return vectors.tolist()

    samples = [
        ("ordinary", b"Fix lock ordering\n\nAcquire worker locks in a consistent order.\n", [b"src/worker.rs", b"tests/locks.rs"]),
        ("short", b"Fix bug", []),
        ("unicode", "Caf\u00e9 e\u0301lan \u4fee\u590d\r\n\nHandle emoji \U0001f680 and na\u00efve paths.\n".encode(), ["src/\u00e9\u4e2d.rs".encode(), b"bad-\xff.rs"]),
        ("invalid-utf8", b"Fix \xff\r\r\n\xfe byte handling", [b"src/\xc3broken.rs"]),
        ("budgets", ("a " * 65 + "\n" + "b " * 400).encode(), [("path/" * 40).encode()]),
        ("wordpiece-boundary", ("unaffordable " * 50 + "\n" + "antidisestablishmentarianism " * 300).encode(), [b"src/continuations.rs"]),
    ]
    documents = []
    for identity, message, paths in samples:
        text, ids, types = document(message, paths)
        documents.append(dict(identity=identity, message=list(message), paths=[list(p) for p in paths],
                              canonical=text, ids=ids, type_ids=types))
    for item, vector in zip(documents, embed([(d["ids"], d["type_ids"]) for d in documents])):
        item["vector"] = vector
    queries = []
    for text in ["avoid deadlock", "Caf\u00e9 e\u0301lan \u4e2d \U0001f680", "", "a " * 254, "a " * 255]:
        content = tokenizer.encode(text, add_special_tokens=False).ids
        if len(content) <= 254:
            encoded = tokenizer.encode(text)
            ids, types = [encoded.ids], [encoded.type_ids]
        else:
            ids, start = [], 0
            while True:
                ids.append([101] + content[start:start + 220] + [102])
                if start + 220 >= len(content):
                    break
                start += 180
            types = [[0] * len(tokens) for tokens in ids]
        vectors = embed(list(zip(ids, types)))
        queries.append(dict(text=text, ids=ids, type_ids=types, vectors=vectors))
    return dict(reference=dict(python=sys.version.split()[0], numpy=np.__version__,
                               onnxruntime=ort.__version__, tokenizers=tokenizers.__version__,
                               provider="CPUExecutionProvider", intra_threads=1, inter_threads=1),
                max_absolute_error=3e-5, max_cosine_distance=1e-6,
                documents=documents, queries=queries)


if __name__ == "__main__":
    result = generate(Path(sys.argv[1]))
    Path(sys.argv[2]).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
