// Standalone uniffi-bindgen binary. Driven by the OntologyCore.xcframework
// build script to emit the Swift module + C header wrapping libontology_core_ffi.
//
// Usage: `cargo run --bin uniffi-bindgen -- generate --library
// target/release/libontology_core_ffi.dylib --language swift --out-dir <dir>`

fn main() {
    uniffi::uniffi_bindgen_main()
}
