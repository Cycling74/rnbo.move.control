# rnbo-move-control

## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
mkdir -p ./target/aarch64-unknown-linux-gnu/debug/deps ./target/aarch64-unknown-linux-gnu/release/deps
cp ../jack2.move/destdir/data/UserData/rnbo/lib/libjack.so* ./target/aarch64-unknown-linux-gnu/debug/deps/
cp ./target/aarch64-unknown-linux-gnu/debug/deps/libjack.so* ./target/aarch64-unknown-linux-gnu/release/deps/

rustup target add aarch64-unknown-linux-gnu
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.12`

## Build

debug:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```

release:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu release
```

## Building in docker image


copy lib contents from jack2.move destdir into /usr/local/oecore-x86_64/sysroots/cortexa72-oe-linux/lib/

install rustup and 

```
rustup target add aarch64-unknown-linux-gnu
```

```
PKG_CONFIG_SYSROOT_DIR=/usr/local/oecore-x86_64/sysroots/cortexa72-oe-linux/ PKG_CONFIG_PATH=/usr/local/oecore-x86_64/sysroots/cortexa72-oe-linux/lib/pkgconfig/ cargo build --target=aarch64-unknown-linux-gnu --release
```

```
conan create .  -s os=Linux -s arch=armv8 -s compiler=gcc -s compiler.version=11.4 -s compiler.libcxx=libstdc++11
```

## Notes

Could this be made more generic and do display/control for various other displays on the rpi?
Could take OSC messages that could be driven by RNBO patches.

```
/rnbo/control/display/sets
/rnbo/control/display/params
/rnbo/control/display/inst

/rnbo/control/nav/next
/rnbo/control/nav/prev
/rnbo/control/load
/rnbo/control/unload

```
