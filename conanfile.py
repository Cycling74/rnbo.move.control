from conans import ConanFile, tools
import os
import re

JACK_VERSION = "1.9.22-457eee49"

BUILD_SCRIPT = r""" #!/bin/bash

. "$HOME/.cargo/env"
cd /build/

CONAN_SETTINGS="-s os=Linux -s arch=armv8 -s compiler=gcc -s compiler.version=11.4 -s compiler.libcxx=libstdc++11"

export CONAN_NON_INTERACTIVE=1
conan remote add -f cycling-public https://conan-public.cycling74.com
conan install jack/{JACK_VERSION}@c74/move ${{CONAN_SETTINGS}}
JACK_PACKAGE_FOLDER=$(conan info --only package_folder --paths jack/{JACK_VERSION}@c74/move ${{CONAN_SETTINGS}} | grep package_folder | sed 's/^[ \t]*package_folder:[ \t]*\(.*\)/\1/')

echo "JACK_PACKAGE_FOLDER ${{JACK_PACKAGE_FOLDER}}"

PKG_CONFIG_SYSROOT_DIR=${{JACK_PACKAGE_FOLDER}} \
PKG_CONFIG_PATH=${{JACK_PACKAGE_FOLDER}}/lib/pkgconfig \
cargo build --target=aarch64-unknown-linux-gnu --release \
--config './.cargo/config-docker.toml' \
--config 'target.aarch64-unknown-linux-gnu.linker="/usr/local/oecore-x86_64/sysroots/x86_64-oesdk-linux/usr/bin/aarch64-oe-linux/aarch64-oe-linux-gcc"' \
--config "target.aarch64-unknown-linux-gnu.rustflags=[\"-C\", \"link-arg=-Wl,-rpath,/data/UserData/rnbo/lib/\", \"-C\", \"link-arg=--sysroot=/usr/local/oecore-x86_64/sysroots/cortexa72-oe-linux\", \"-C\", \"link-arg=-L${{JACK_PACKAGE_FOLDER}}/lib/\"]"
""".format(JACK_VERSION=JACK_VERSION)

class RNBOMoveControl(ConanFile):
	name = "rnbomovecontrol"
	exports_sources = "src/*", "Cargo.*", ".cargo/*", "config/**"

	#common
	user = "c74"
	channel = "move"
	settings = { "os": ["Linux"], "compiler": {"gcc": {"version": ["11.4"], "libcxx": "libstdc++11"}}, "arch": "armv8" }
	options = { "dockerimage": ["ANY"] }
	default_options = { "dockerimage": "rnbo.move.takeover:0.1" }

	def set_version(self):
		with open("Cargo.toml") as f:
			content = f.read().splitlines()
			for line in content:
				m = re.search(r'version\s*=\s*"(.*)"', line)
				if m:
					self.version = m.group(1)
					return
		raise Exception("cannot find version info in Cargo.toml")

	def build(self):
		with open(os.path.join(self.source_folder, "build.sh"), "w") as f:
			f.write(BUILD_SCRIPT)
		self.run("docker run --user node -v $(pwd):/build --platform linux/amd64 %s /bin/bash /build/build.sh" % self.options.dockerimage, cwd=self.source_folder)

	def package(self):
		self.copy("target/aarch64-unknown-linux-gnu/release/rnbomovecontrol", dst="bin")
