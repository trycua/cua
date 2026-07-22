/**
 * Rust-backed SDK for Cua Driver client applications.
 *
 * Agents should configure `cua-driver mcp` through their runtime's existing
 * MCP client instead of importing a language MCP facade.
 */
export * from "./native/index.js"
export { default } from "./native/index.js"
