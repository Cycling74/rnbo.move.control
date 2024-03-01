# rnbo-move-control

## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
mkdir -p ./target/aarch64-unknown-linux-gnu/debug/deps ./target/aarch64-unknown-linux-gnu/release/deps
scp -O move:"jack2/lib/libjack.so*" ./target/aarch64-unknown-linux-gnu/debug/deps/
cp ./target/aarch64-unknown-linux-gnu/debug/deps/libjack.so* ./target/aarch64-unknown-linux-gnu/release/deps/
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.2`

## Build

debug:
```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.2/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```

release:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.2/sysroot/ cargo build --target=aarch64-unknown-linux-gnu --release && scp -O ./config/rnbomovecontrol-init.d ./target/aarch64-unknown-linux-gnu/release/rnbomovecontrol move:rnbo/
```
