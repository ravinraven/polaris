# Contributing

## About this fork

This is a personal fork of [agersant/polaris](https://github.com/agersant/polaris). The upstream project is not open to code contributions, so this fork exists to take Polaris in a direction that fits the maintainer's own use case (for example, support for additional audio formats such as `webm`). It is paired with a fork of the web client, [ravinraven/polaris-web](https://github.com/ravinraven/polaris-web).

## Guidelines

Issues and pull requests are welcome here. A few things to keep in mind:

- **Response times vary.** This is a side project, so it may take a while for issues and pull requests to get a response or review. Please be patient.
- **Ideas are welcome.** Feature requests and suggestions are appreciated, even if you aren't able to implement them. If you plan to work on something sizeable, consider opening an issue first to discuss it.
- **Keep changes focused.** Small, self-contained pull requests are easier to review. Include tests where it makes sense (see below).
- **Bugs in upstream behavior** that are not specific to this fork may be better reported to [agersant/polaris](https://github.com/agersant/polaris), subject to its own contribution policy.
- **Other ways to help:** answer questions in the issue tracker, package this fork, or maintain your own fork of it.

## Compiling and running Polaris

1. [Install Rust](https://www.rust-lang.org/en-US/install.html) (stable toolchain)
2. Clone this repository with this command: `git clone https://github.com/ravinraven/polaris.git`
3. You can now run compile and run polaris from the newly created directory with the command: `cargo run`

Polaris supports a few command line arguments which are useful during development:

- `-c some/config.toml` sets the location of the [configuration](/docs/CONFIGURATION.md) file.
- `--data some/path` sets the folder Polaris will use to store runtime data such as playlists, collection index and auth secrets.
- `-w some/path/to/web/dir` lets you point to the directory to be served as the web interface. You can find a suitable directory in your Polaris install (under `/web`), or from the [latest polaris-web release](https://github.com/ravinraven/polaris-web/releases/latest/download/web.zip), or build your own from the [polaris-web fork](https://github.com/ravinraven/polaris-web).
- `-f` (on Linux) makes Polaris not fork into a separate process.

Putting it all together, a typical command to compile and run the program would be: `cargo run -- -w web -c test-config.toml`

While Polaris is running, access the web UI at [http://localhost:5050](http://localhost:5050).

## Running unit tests
Run `cargo test`!

## Building the container image

Run `podman build -t polaris .` from the root of the repository (`docker build` works too). The image is built from the [Containerfile](Containerfile), and its entrypoint is [res/container/run-polaris](res/container/run-polaris). If you add or rename a command line option that the entrypoint passes to Polaris (`-f`, `-p`, `-c`, `--data`, `--cache`, `-w`, `--log-level`), update that script and the [container documentation](docs/CONTAINER.md). Run `podman run --rm polaris --show-paths` for a quick check that the entrypoint arguments are still accepted.
