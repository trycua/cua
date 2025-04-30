#!/usr/bin/env bash

echo "Creating helloworld.txt on the Desktop..."
if [ ! -f ~/Desktop/helloworld.txt ]; then
  echo "Hello, World!" > ~/Desktop/helloworld.txt
  echo "helloworld.txt created successfully."
else
  echo "helloworld.txt already exists."
fi