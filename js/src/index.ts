/**
 * Bramble wallet core for JavaScript hosts. Await `uniffiInitAsync` once, with the module's
 * `.wasm` (a URL, a `fetch()` promise, bytes, or a compiled `WebAssembly.Module`), before any call.
 */
export * from "./generated/index.js";
