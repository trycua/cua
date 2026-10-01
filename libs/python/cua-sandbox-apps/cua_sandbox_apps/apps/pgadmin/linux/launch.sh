#!/bin/bash
# Launch pgAdmin4 in Desktop Mode

INSTALL_DIR="/opt/pgadmin4"

# Set up environment for desktop mode
export PGADMIN_CONFIG_ENHANCED_COOKIE_PROTECTION=False
export PGADMIN_CONFIG_COOKIE_DEFAULT=False
export PGADMIN_CONFIG_COOKIE_SAMESITE=Lax
export PGADMIN_CONFIG_PROXY_X_FOR_COUNT=1
export PGADMIN_CONFIG_PROXY_X_PROTO_COUNT=1
export PGADMIN_CONFIG_PROXY_X_HOST_COUNT=1

# Launch pgAdmin4 with initial user credentials
bash -c "source $INSTALL_DIR/venv/bin/activate && (echo 'admin@example.com'; echo 'pgadmin123'; echo 'pgadmin123') | timeout 120 pgadmin4"