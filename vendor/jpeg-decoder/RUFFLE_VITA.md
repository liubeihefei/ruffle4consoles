# Vita memory patch

This directory vendors `jpeg-decoder 0.3.2` from crates.io. The original crate archive SHA-256 is
`00810F1D8B74BE64B13DBF3DB89AC67740615D6C891F0E7B6179326533011A07`, matching the checksum in the
pre-vendor `Cargo.lock`.

The Vita patch adds `Decoder::decode_into` so a caller can reserve the final RGBA capacity before
JPEG component decoding and write RGB24 into that allocation. Ruffle then expands RGB to RGBA in
place from the end of the buffer. Other decoder behavior is unchanged.
