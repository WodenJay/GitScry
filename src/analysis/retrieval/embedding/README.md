# Pinned offline embedding module (#111)

This directory is a reusable Rust crate, deliberately not wired into the product
manifest yet. The dependent indexing/search tickets can add a path dependency.
Existing CLI builds therefore neither link ORT nor initialize/download a model.
This is a real encoder used by `tools/encoder-gate`, not a second toy runtime.

## Interface and organization

- `Encoder::open(resource_directory, Workload::{Index,Query})`: verify all six
  resources first; create one CPU-only sequential session with inter-op threads
  1 and intra-op threads `min(available_parallelism, 10/4)`.
- `documents(&[Document])`: raw message/path bytes plus identity; ordered
  `(identity, [f32;384])` results. Paths must already have canonical caller order.
- `query(&str)`: ordered window vectors; no ranking or cache behavior here.

The external seam is this encoder interface. `protocol.rs` owns byte replacement,
field construction, original-text offset truncation, and original-ID query windows;
`runtime.rs` owns bounded execution and order restoration; `pooling.rs` owns mask
and numerical validation; `resources.rs` owns pinned offline file verification.
No provider framework, GPU, CLI, storage, download, or progress abstraction.

Documents use `Title: ...\nBody: ...\nPaths: ...`, 64 title / 32 path content-token
budgets, then the body prefix within 256 total tokens. The final encoding is
rechecked at field boundaries. Message splitting is at the first newline; only
trailing CRs on the title are removed. Invalid UTF-8 is replaced, not rejected.
Exceptional bodies are prefix-tokenized instead of tokenizing the entire body.
Short queries allow 254 content tokens + CLS/SEP; longer queries slice the
original IDs into 220-token windows, overlap 40, without decoding/re-tokenizing.
At most 32 windows (5800 content tokens) are accepted; larger queries fail before
inference with shortening guidance, rather than silently truncating the query.

Eight documents at most are encoded/inferred at a time. Length ordering is local
to each batch, and input identities/order are restored. Padding is right-hand zero
IDs/types and zero attention mask. Mean pooling includes special tokens but
excludes padding; accumulation matches the Python reference's f64 masked sum,
then FP32 conversion and L2 normalization. Reject malformed, zero, or nonfinite
outputs. Sessions are reused; no concurrent batches or resource acquisition.

## Model-free tests and reference provenance

```
cargo test --manifest-path src/analysis/retrieval/embedding/Cargo.toml --locked --no-default-features --jobs 1
```

The no-default-feature tests exercise protocol, pooling, resource identity, query
boundaries/coverage, and a 20MB body without loading ORT or downloading a model.
`fixtures/tokenizer.json` is the pinned unmodified tokenizer (Apache-2.0 model
resources); its hash/size is checked against `resources.json`.
`fixtures/reference.json` contains independent Python token/vector fixtures;
no model weights are checked into Git. `tools/encoder-gate/reference.py` reproduces
#109's original `embed_corpus.py` protocol and records package versions. Run it
with a verified resource directory and destination filename. It never downloads
or installs packages. If packages are needed, use a project-local uv `.venv`.

Reference comparisons require maximum absolute component error <= **3e-5** AND
cosine distance <= **1e-6**, with norm error <= **1e-5**. This explicitly allows
FP32/platform differences rather than promising byte equality. Fixtures include
ordinary/short documents, Unicode/combining characters, malformed UTF-8, split
WordPieces, field limits, empty queries, and 254/255 content-token queries.
The release harness reverses/repeats inputs across batch boundaries, compares
single vs padded batches, checks identities, and rejects corrupt resources.

For pinned native runtime source/linking recipes and per-platform evidence, see
[`tools/encoder-gate/README.md`](../../../../tools/encoder-gate/README.md).
Native `runtime` is default; local model-free tests intentionally disable it.
Do not use `load-dynamic`, downloaded prebuilt runtimes, or source patches.
