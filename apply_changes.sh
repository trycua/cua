#!/bin/bash
set -e
sed -i.bak 's/eval(/ast.literal_eval(/g' libs/python/agent/agent/loops/uitars.py && rm libs/python/agent/agent/loops/uitars.py.bak