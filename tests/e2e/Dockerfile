FROM debian:trixie-slim

RUN apt-get update && apt-get install -y curl ca-certificates git && rm -rf /var/lib/apt/lists/*

# Install CUE
RUN curl -fsSL https://github.com/cue-lang/cue/releases/download/v0.16.0/cue_v0.16.0_linux_amd64.tar.gz \
    | tar -xz -C /usr/local/bin cue

# Binaries are pre-built on host and copied in
COPY bin/forest /usr/local/bin/forest
COPY bin/forest-server /usr/local/bin/forest-server
RUN chmod +x /usr/local/bin/forest /usr/local/bin/forest-server

COPY run.sh /e2e/run.sh
RUN chmod +x /e2e/run.sh

WORKDIR /e2e
ENTRYPOINT ["/e2e/run.sh"]
