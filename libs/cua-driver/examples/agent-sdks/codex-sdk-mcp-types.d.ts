// @openai/codex-sdk exposes this upstream type without declaring the MCP SDK
// as a package dependency. The example only reads Codex's final text, so this
// structural declaration is sufficient and avoids adding an unused runtime.
declare module '@modelcontextprotocol/sdk/types.js' {
  export interface ContentBlock {
    type: string;
    [key: string]: unknown;
  }
}
