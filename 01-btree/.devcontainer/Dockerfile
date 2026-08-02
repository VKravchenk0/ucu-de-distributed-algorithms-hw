FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

RUN apt-get update && \
    apt-get install -y \
        pkg-config \
        libssl-dev \
        build-essential \
        && rm -rf /var/lib/apt/lists/*
