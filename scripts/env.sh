# Source this before building. Uses the distro Rust 1.97 toolchain + build-std for Windows cross builds.
export PATH=/usr/lib/rust-1.97/bin:$PATH
export RUSTC_BOOTSTRAP=1
export WINEDEBUG=-all
export WINEPREFIX=/tmp/wineprefix
WIN_TARGET=x86_64-pc-windows-gnu
win_build() { cargo build --release --target $WIN_TARGET -Zbuild-std=std,panic_unwind "$@"; }
win_check() { cargo check --target $WIN_TARGET -Zbuild-std=std,panic_unwind "$@"; }
