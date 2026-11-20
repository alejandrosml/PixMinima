use miniz_oxide::inflate::{
    TINFLStatus,
    core::{DecompressorOxide, decompress, inflate_flags::*},
};

pub(super) fn inflate_exact(blob: &[u8], len: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; len + 1];
    let (status, consumed, produced) = decompress(
        &mut DecompressorOxide::new(),
        blob,
        &mut out,
        0,
        TINFL_FLAG_PARSE_ZLIB_HEADER
            | TINFL_FLAG_COMPUTE_ADLER32
            | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    if status != TINFLStatus::Done || produced != len || consumed != blob.len() {
        return Err("ZIP payload length or checksum mismatch".into());
    }
    out.truncate(len);
    Ok(out)
}

// Keep the DEFLATE stream itself independent of each container's predictors.
pub(super) fn deflate(data: &[u8], thorough: bool) -> Vec<u8> {
    let fast = miniz_oxide::deflate::compress_to_vec_zlib(data, if thorough { 9 } else { 6 });
    if thorough
        && let Ok(slow) = pixminima_png::compress_zlib_thorough(data)
        && slow.len() < fast.len()
    {
        return slow;
    }
    fast
}
