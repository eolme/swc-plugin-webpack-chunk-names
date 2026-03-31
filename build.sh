set -ex

mkdir -p lib

export CARGO_UPDATE_LOCK=false
export RUST_LOG=off
export RUSTFLAGS="-Zfmt-debug=none -Zunstable-options -Zlocation-detail=none -Cpanic=immediate-abort --cfg=swc_ast_unknown -Clink-arg=--gc-sections -Clink-arg=--strip-all -Clink-arg=--strip-debug -Ctarget-feature=+bulk-memory,+multivalue,+mutable-globals,+nontrapping-fptoint,+reference-types,+sign-ext,+simd128"

cargo build \
    --release \
    --target wasm32-wasip1 \
    -Z build-std=std,core,alloc,panic_abort \
    -Z build-std-features=optimize_for_size

wasm-opt \
    --converge \
    --enable-simd \
    --enable-nontrapping-float-to-int \
    --enable-bulk-memory \
    --enable-sign-ext \
    --enable-mutable-globals \
    --strip-debug \
    --strip-dwarf \
    --strip-producers \
    --strip-target-features \
    --vacuum \
    --emit-exnref \
    --flatten --rereloop -Oz -Oz \
    target/wasm32-wasip1/release/swc_plugin_webpack_chunk_names.wasm \
    -o lib/swc_plugin_webpack_chunk_names.wasm
