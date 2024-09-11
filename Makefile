configure:
	@which cargo > /dev/null || (echo "Please install Rust 1.70 (or newer) toolchain; e.g. with 'apt install cargo', or using rustup" && exit 1)

all: configure
	cargo build
