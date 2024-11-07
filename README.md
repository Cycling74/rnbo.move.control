# rnbo-move-control

## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
mkdir -p ./target/aarch64-unknown-linux-gnu/debug/deps ./target/aarch64-unknown-linux-gnu/release/deps
scp -O move:"jack2/lib/libjack.so*" ./target/aarch64-unknown-linux-gnu/debug/deps/
cp ./target/aarch64-unknown-linux-gnu/debug/deps/libjack.so* ./target/aarch64-unknown-linux-gnu/release/deps/
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.12`

## Build

debug:
```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```

release:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu --release && scp -O ./config/rnbomovecontrol-init.d ./target/aarch64-unknown-linux-gnu/release/rnbomovecontrol move:rnbo/
```

## Install

ssh to your move as root
```
ssh root@move
cp /data/UserData/rnbo/rnbomovecontrol-init.d /etc/init.d/rnbomovecontrol
pushd /etc/rc5.d/
ln -s ../init.d/rnbomovecontrol S95rnbomovecontrol
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
