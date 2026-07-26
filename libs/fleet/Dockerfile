# Build stage — produces /app/dist (Vite output).
FROM node:20-alpine AS builder
WORKDIR /app

# pnpm via corepack (matches the repo's pnpm-based workflow).
RUN corepack enable && corepack prepare pnpm@10.12.3 --activate

# Install deps first (better layer caching). patches/ must be present before
# install so pnpm can apply pnpm.patchedDependencies (e.g. the @novnc BGR fix).
COPY package.json pnpm-lock.yaml ./
COPY patches ./patches
RUN pnpm install --frozen-lockfile --ignore-workspace

# Build.
COPY . .
RUN pnpm build

# Runtime stage — nginx serves static files and proxies /api to the backend.
FROM nginx:1.27-alpine AS runtime

# In-cluster DNS for the cyclops-cs-backend Service (sibling Deployment
# in the same namespace). Override if the backend is reached at a
# different hostname.
#
# Use the short Service name, NOT the FQDN. nginx:alpine uses musl;
# with ndots:5 the 4-dot FQDN can't be resolved at config-parse time
# and nginx crashloops with "host not found in upstream". The short
# name resolves through the search list at runtime.
ENV CYCLOPS_CS_BACKEND=http://cyclops-cs-backend:8080

# Default Keycloak runtime config for the SPA. Overridden via the
# CYCLOPS_CS_CONFIG_JSON env var (JSON object as a string) at deploy
# time — nginx envsubst inlines it into /config.js, which the SPA
# loads before the React bundle (window.__CYCLOPS_CS_CFG__).
ENV CYCLOPS_CS_CONFIG_JSON='{"kcUrl":"https://auth.cua.ai","kcRealm":"cyclops-cs","kcClientId":"cyclops-cs-spa"}'

COPY nginx.conf /etc/nginx/templates/default.conf.template
COPY --from=builder /app/dist /usr/share/nginx/html
