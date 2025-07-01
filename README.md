# rnbo-move-control

## Setting up dev on osx

```
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
```

Mount the AOS SDK, in this case: `SDK-toolchain-abletonos-aarch64-rpi4-v3.12`

## Creating Font Files

check out:

* https://github.com/farsil/ibmfonts.git
* https://github.com/embedded-graphics/bdf.git
  * `cd location/of/bdf/eg-font-converter`
  * `cargo build --release`
* in the root of this current project, assuming these are both checked out in `~/local/src/`

```
mkdir -p src/font/
~/local/src/bdf/target/release/eg-font-converter --data src/font/cga_8x16.data --rust src/font/cga8x16.rs ~/local/src/ibmfonts/bdf/ic8x16u.bdf CGA_8X16
~/local/src/bdf/target/release/eg-font-converter --data src/font/cga_light_8x16.data --rust src/font/cgalight8x16.rs ~/local/src/ibmfonts/bdf/icl8x16u.bdf CGA_LIGHT_8X16
```


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

## OSC

* `/rnboctl/view/display <view index> [<optional page index>]`
  * send this message and no matter where you are, the display should show the given view (clamped to be in range) at either page 0 or the page you specify (clamped to be in range)
  * this message should also be sent from the control app to indicate what view/page it is displaying
* `/rnboctl/view/page <page index>`
  * send this message if you're in a param view and the view should display the page you've given (clamped to be in range)


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
