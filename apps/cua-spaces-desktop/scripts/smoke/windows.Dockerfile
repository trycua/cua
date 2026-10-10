# Tools for the Windows installer structure check (scripts/smoke/windows.sh).
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      7zip file python3 python3-pefile nodejs \
    && rm -rf /var/lib/apt/lists/*
