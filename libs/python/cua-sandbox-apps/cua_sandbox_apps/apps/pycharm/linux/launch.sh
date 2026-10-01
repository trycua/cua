#!/bin/bash

# PyCharm Launch Script
export DISPLAY=:1
/opt/pycharm/bin/pycharm.sh > /tmp/pycharm.out 2>&1 &
echo $!