# The official MCP reference "everything" server over streamable HTTP on
# :3001 (/mcp), for the opt-in live tests in tests/mcp_live.rs.
FROM node:22-slim
RUN npm install -g @modelcontextprotocol/server-everything@2026.8.31 && npm cache clean --force
USER node
ENV PORT=3001
EXPOSE 3001
CMD ["mcp-server-everything", "streamableHttp"]
