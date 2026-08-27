set -e

# Wrapped Allocators seem to cause issues with touch and don't seem to help with the OOMs 
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=malloc"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=free"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=calloc"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=realloc"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=memalign"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=memcpy"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,--wrap=memset"
#export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-Wl,-q"
export RUSTFLAGS="$RUSTFLAGS -C target-cpu=cortex-a9"
cargo +nightly vita build elf --profile=vita --locked
cp target/armv7-sony-vita-newlibeabihf/vita/ruffle4consoles.elf \
    /tmp/ruffle4consoles.debug.elf
cargo +nightly vita build vpk --profile=vita --locked
gzip -1 -c /tmp/ruffle4consoles.debug.elf > \
    target/armv7-sony-vita-newlibeabihf/vita/ruffle4consoles.debug.elf.gz
test -s target/armv7-sony-vita-newlibeabihf/vita/ruffle4consoles.debug.elf.gz
