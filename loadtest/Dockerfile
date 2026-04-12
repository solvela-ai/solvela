FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY solvela /usr/local/bin/solvela
RUN chmod +x /usr/local/bin/solvela
ENTRYPOINT ["sleep", "infinity"]
