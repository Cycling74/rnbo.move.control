from conans import ConanFile, tools
import os
import re

class RNBOMoveControl(ConanFile):
	name = "rnbomovecontrol"
	exports_sources = "src/*", "Cargo.*", ".cargo/*", "config/**", "content/*"

	#common
	user = "c74"
	channel = "move"
	settings = { "os": ["Linux"], "arch": "armv8" }
	options = { "dockerimage": ["ANY"] }
	default_options = { "dockerimage": "xnor/rnbo-runner-xpile:0.1" }

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
		self.run(
			"docker run"
			" --user root"
			" --platform linux/amd64"
			" -v $(pwd):/build"
			" -v cargo-registry:/usr/local/cargo/registry"
			" -v cargo-git:/usr/local/cargo/git"
			" %s"
			" bash -c 'cd /build && PKG_CONFIG_SYSROOT_DIR=/usr/lib/aarch64-linux-gnu/ PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig/ cargo build --target=aarch64-unknown-linux-gnu --config=./.cargo/config-docker.toml --release'"
			% self.options.dockerimage,
			cwd=self.source_folder
		)

	def package(self):
		self.copy("rnbomovecontrol", dst="bin", src="target/aarch64-unknown-linux-gnu/release/")
		self.copy("config/control-startup.json")
