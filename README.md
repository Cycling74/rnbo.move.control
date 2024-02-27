## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
```

## Build

```
cargo build --target=aarch64-unknown-linux-gnu
```
