# rnbo-move-control

## Bugs

* `Patchers` list not updating when sending new patchers

## Attribution

* fonts:
    * [spleen](https://github.com/fcambus/spleen)

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
~/local/src/bdf/target/release/eg-font-converter --data src/font/spleen_8x16.data --rust src/font/spleen8x16.rs ~/local/src/spleen/spleen-8x16.bdf SPLEEN_8X16
```


## Build

debug:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu
```

release:

```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu --release
```

release and send
```
PKG_CONFIG_SYSROOT_DIR=/Volumes/SDK-toolchain-abletonos-aarch64-rpi4-v3.12/sysroot/ cargo build --target=aarch64-unknown-linux-gnu --release && scp ./target/aarch64-unknown-linux-gnu/release/rnbomovecontrol move-usb:rnbo/bin
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
* `/rnboctl/device/params <device index> [<optional page index>]`
  * send this message to display the parameters for the device that has the given instance index, optionally at the specified 0-indexed page
* `/rnboctl/device/data <device index>`
  * send this message to display the data page for the device that has the given instance index

* `/rnboctl/userview/display <index>`
  * send this message to display the user drawn view with index 0
  * TODO: use a 2nd index to indicate the display to draw it on
* `/rnboctl/userview/layer/hide <index> <layer> 1/0`
  * send this message to selectively hide or reveal a layer, `1` to hide, `0` to reveal
* `/rnboctl/userview/layer/xor <index> <layer> 1/0`
  * send this message to selectively change the rendering style of a layer. `1` to xor and `0` to sum (default)
* `/rnboctl/display/count`
  * send an empty message here to get a number echoed back
* `/rnboctl/display/info <index>`
  * send a message here with the index to get details about the display echoed back
  * response will come to the same endpoint in the format:
    * index, pixel width, pixel height, color format, frame period in milliseconds
    * current color formats:
      * 0: 1-bit black and white
* `/rnboctl/userview/layer/redraw <index> <layer>`
  * send this message to tell the controller to re-read and draw a layer, useful when waveform contents change

* `/rnboctl/param/delta <value>`
  * send a floating point value to change global delta that gets used while editing parameters
    * this value is used to increment/decrement the normalized value
    * this does not affect parameters that have specific deltas set up via metadata
    * this also does not affect enum or stepped parameters
  * send an empty value to this address to query the current value

* `/rnboctl/redraw`
  * send this to redraw the takeover UI, useful after sending MIDI reset

TODO:

* `/rnboctl/userview/zoomfull <index>`
  * send this message to display the user drawn view with index 0
  * TODO: use a 2nd index to indicate the display to draw it on


## Parameters

### Delta

There is a default delta when editing parameters via the encoders that you can
alter via the `rnboctl` OSC message detailed above.

If you want to set a different delta for a specific parameter you can set it
via that parameter's meta entry, the key is `"delta"` and the value should be
floating point.

### Hidden

You can hide parameters from the display via a `"hidden"` boolean value in that parameter's `meta` entry.


## User Views

Users can draw to the display using shared memory and buffers.

### Metadata

To create a view, you must add a meta entry to a buffer. You can let the
controller render your buffer contents as a waveform or use the "direct
drawing" approach. Both are detailed below.

Here is an example metadata with all of the fields that define a view.

```
{ "view": 0, "viewname": "foo", "z": 1, "viewhidden": false, "system": true }
```

* `view` - defines the view and gives an index for selecting this via the `/rnboctl/userview/display` OSC message.
* `system` - this boolean indicates if the buffer should be shared via shared memory so that other processes can access it. `system` sharing is how views are seen by the RNBO control application.
* `z` - this defines a 'z' index to use while rendering the view, you can have multiple buffers share the same `view` index and then the `z` order will define how the buffer contents are layered, with higher values covering lower ones.
* `viewname` - an optional name for the view, the first `viewname` seen for a specific `view` index will be the one that is used if there are multiple. If you provide a `viewname` you should see it in the `User Views` submenu, which you can access from the top level menu on the Move. You'll only see the name though if you have more than 1 view because otherwise we just jump direct to the view instead of displaying a sub menu.
* `viewhidden` - this is a boolean value that indicates if the associated data should be hidden initially or not. The default is false if it isn't supplied.
* `viewxor` - this is a boolean value that indicates if the associated data should xor with the data in lower layers. This sets white to black and black to white for each bit set in this layer.
* `paramview` - this string value indicates a `Parameter View` name that should be loaded and available while the view is loaded.
* `share` - this string value indicates a name that allows for buffers to be shared between devices in the graph.
* `observe` - this string value indicates a `share` name that this buffer should follow.
* `display` - this string value indicates a name of a display, use `"web"` to create a streaming video via the web interface
* `displaysize` - a 2 element numeric array: `[width, height]` representing the width and height of the display to render

### Direct Drawing

You can render bits directly to a buffer and have the controller interpret that
as an image. This gives you the option to draw arbitrary, even animated, images
to the screen.

To do Direct Drawing, you must use the `UInt8` type for your buffer and provide
the `view` and `"system": true` metadata entries.

The buffer is interpreted as a 32 byte header followed by pixel data.

#### Header Format

* The first byte of the header is a dirty flag, atomically look for it to be
  clear, then you write pixel data, then atomically set it to 1. When your view
  is visible, the controller will look for this 1, read the data, then
  atomically clear that byte.  The controller does not access this data unless
  it is displaying it.
* The 2nd byte of the header indicates the pixel format of your image.
  * `0` - 1 bit per pixel black and white
  * `1` - 8-bits per channel, RGB, 24-bits total per pixel
* The 3rd and 4th bytes are reserved.
* The 5th thru 12th bytes are treated as 2 32-bit little endian unsigned ints,
  the first represents the width and second the height of the image.
* The 13th thru 24th bytes represent 2 32-bit little endian signed ints,
  the first an x and the second a y, offset for placing the view on the screen.
* the rest are reserved

```
00..03 - dirty, format, reserved, reserved
04..07 - width (32-bit unsigned int)
08..11 - height (32-bit unsigned int)
12..15 - x offset (32-bit signed int)
16..23 - y offset (32-bit signed int)
.. reserved
```


### Waveform rendering

**TODO**


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
