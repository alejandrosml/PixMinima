use super::limits::{MAX_FILE, MAX_METADATA, MAX_PIXELS, check_container, check_jpeg};
use zenjxl_decoder::api::{JxlDecoderLimits, JxlDecoderOptions};

pub fn archive_jpeg(jpeg: &[u8]) -> Result<Vec<u8>, String> {
    check_jpeg(jpeg)?;
    let jxl = std::panic::catch_unwind(|| {
        let config = jixel::JpegTranscodeConfig::default()
            .with_num_threads(1)
            .with_jpeg_reconstruction(true);
        jixel::encode_jpeg_lossless_with_config(jpeg, &config)
    })
    .map_err(|_| "JPEG XL encoder could not process this JPEG")?
    .map_err(|e| format!("JPEG XL encoding: {e}"))?;
    if restore_jpeg(&jxl)? != jpeg {
        return Err("JPEG XL round-trip did not reproduce every original byte".into());
    }
    Ok(jxl)
}

pub fn restore_jpeg(jxl: &[u8]) -> Result<Vec<u8>, String> {
    check_container(jxl)?;
    std::panic::catch_unwind(|| reconstruct(jxl))
        .map_err(|_| "JPEG XL decoder could not process this archive".to_owned())?
}

fn reconstruct(jxl: &[u8]) -> Result<Vec<u8>, String> {
    let mut limits = JxlDecoderLimits::restrictive();
    limits.max_pixels = Some(MAX_PIXELS);
    limits.max_extra_channels = Some(0);
    limits.max_icc_size = Some(MAX_METADATA);
    limits.max_memory_bytes = Some(512 << 20);
    let mut options = JxlDecoderOptions::default();
    options.limits = limits;
    options.parallel = false;
    options.skip_preview = true;
    let jpeg = zenjxl_decoder::reconstruct_jpeg_with(jxl, options)
        .map_err(|e| format!("JPEG XL reconstruction: {e}"))?
        .ok_or("This JPEG XL image does not contain a restorable JPEG")?;
    if jpeg.len() > MAX_FILE {
        return Err("Restored JPEG exceeds the archive limit".into());
    }
    check_jpeg(&jpeg)?;
    Ok(jpeg)
}
