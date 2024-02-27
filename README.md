## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
mkdir -p ./target/debug/deps
scp -O move:"jack2/lib/libjack.so*" ./target/debug/deps
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.2`

## Build

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.2/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```
