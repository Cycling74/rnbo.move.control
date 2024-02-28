## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
mkdir -p ./target/debug/deps ./target/release/deps
scp -O move:"jack2/lib/libjack.so*" ./target/debug/deps/
cp ./target/debug/deps/libjack.so* ./target/release/deps/
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.2`

## Build

debug:
```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.2/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```

release:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.2/sysroot/ cargo build --target=aarch64-unknown-linux-gnu --release && scp -O ./target/aarch64-unknown-linux-gnu/release/move-control move:rnbo/
```
