#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: linux_session.sh [--evidence DIR] [--check-run DIR]... -- COMMAND [ARG...]
#
# Runs COMMAND inside a headless X11 session (Xvfb 1920x1080x24, private D-Bus,
# openbox, picom/xrender). Evidence (launch.log, component logs, at-spi-address.txt,
# pids.jsonl, exit) goes to a directory outside any git checkout. Only processes this
# supervisor (linux_session.py) started are stopped. COMMAND's exit status passes through; a leaked process
# or incomplete --check-run evidence turns a zero status into 1.
exec python3 "$(dirname "${BASH_SOURCE[0]}")/linux_session.py" "$@"
