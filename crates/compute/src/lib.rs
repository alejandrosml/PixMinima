#![forbid(unsafe_code)]

mod cpu;
#[cfg(all(target_os = "macos", feature = "metal"))]
mod metal;
#[cfg(all(target_os = "macos", feature = "metal"))]
pub use metal::share_metal_device;
#[cfg(all(target_os = "macos", feature = "metal"))]
mod automatic;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ComputeMode {
    #[default]
    Auto,
    Cpu,
    Metal,
}

/// Score packed RGBA8 rows. If the requested hardware is unavailable the caller
/// can use the PNG engine's generic reference implementation.
pub fn choose_rows(
    input: &[u8],
    width: usize,
    height: usize,
    mode: ComputeMode,
) -> Option<Vec<u8>> {
    if width == 0
        || height == 0
        || width > 32_768
        || width.checked_mul(height)?.checked_mul(4)? != input.len()
    {
        return None;
    }
    #[cfg(all(target_os = "macos", feature = "metal"))]
    match mode {
        ComputeMode::Metal => return metal::choose(input, width, height),
        ComputeMode::Auto => return Some(automatic::choose(input, width, height)),
        ComputeMode::Cpu => {}
    }
    #[cfg(not(all(target_os = "macos", feature = "metal")))]
    if mode == ComputeMode::Metal {
        return None;
    }
    Some(cpu::rows(input, width))
}
