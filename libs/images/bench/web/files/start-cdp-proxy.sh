#!/usr/bin/env bash
# Chromium binds its DevTools port to loopback only; publish it on 9222.
exec socat TCP-LISTEN:9222,fork,reuseaddr,bind=0.0.0.0 TCP:127.0.0.1:9223
