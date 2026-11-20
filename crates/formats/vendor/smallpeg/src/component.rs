//! Coefficient storage and JPEG sampling geometry.
pub(crate) struct Component {
    pub(crate) blocks: Vec<[i16; 64]>,
    pub(crate) table_class: usize,
    pub(crate) bw: usize,
    pub(crate) aw: usize,
    pub(crate) ah: usize,
    pub(crate) h: u8,
    pub(crate) v: u8,
    pub(crate) qt_id: u8,
}

/// Block emission order for an interleaved scan: per MCU, each component's
/// sampling region left-to-right, top-to-bottom.
pub(crate) fn interleave_order(
    comps: &[Component],
    mcu_w: usize,
    mcu_h: usize,
) -> Vec<(usize, usize)> {
    let mut order = Vec::new();
    for my in 0..mcu_h {
        for mx in 0..mcu_w {
            for (ci, c) in comps.iter().enumerate() {
                let bw = mcu_w * c.h as usize;
                for dy in 0..c.v as usize {
                    for dx in 0..c.h as usize {
                        let bx = mx * c.h as usize + dx;
                        let by = my * c.v as usize + dy;
                        order.push((ci, by * bw + bx));
                    }
                }
            }
        }
    }
    order
}
