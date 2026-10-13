# Running tissue in a container

The PoC runs as a single static binary in a minimal container. The GUI
assets are embedded in the binary, so the final image contains nothing
else — a musl build in a `scratch` final stage.

## Build the image

The repository `Dockerfile` is multi-stage: musl build, then `scratch`
final stage.

```sh
docker build -f Dockerfile -t tissue:poc .
```

CI also builds the image on every push to `main` and uploads it as a
workflow artifact (`tissue-poc-container-image`) if you prefer pulling
over building.

## View an issue repo in the browser

```sh
docker run --rm --network host \
  -v /path/to/myproject-issues:/issues tissue:poc \
  serve --port 8080 --repo /issues
```

Then open `http://127.0.0.1:8080`.

## Read-only CLI through the container

Read-only commands work through the entrypoint:

```sh
docker run --rm -v /path/to/myproject-issues:/issues tissue:poc list --repo /issues
docker run --rm -v /path/to/myproject-issues:/issues tissue:poc show IAM-1 --repo /issues
```

## Why `--network host`, and why loopback-only

The viewer **binds `127.0.0.1` exclusively** and refuses to serve on
any other address — nothing is exposed beyond your machine, by design.
In a container the loopback interface is per-network-namespace, so with
default bridge networking the server would be unreachable. On Linux,
`--network host` shares the host's network namespace: the binary binds
the host's loopback directly, `-p` publishing is unnecessary, and
nothing is exposed to the network. Binding inside a bridge-network
container is unreachable *by design* — the port is never published.

## What belongs on the host

Bootstrap (`tissue init`) runs on the host with a local binary: the
final image contains only the tissue binary, and `init` needs the git
CLI. The container is for **viewing and read-only commands** against an
existing issue repo.

## Next

- [Getting started](getting-started.md) — install and first run on bare metal
- [CLI reference](cli-reference.md) — every command and flag
