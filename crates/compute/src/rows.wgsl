struct Dimensions { width: u32, height: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read> pixels: array<u32>;
@group(0) @binding(1) var<storage, read_write> selected: array<u32>;
@group(0) @binding(2) var<uniform> extent: Dimensions;
var<workgroup> partial: array<array<u32, 5>, 64>;

fn channels(p: u32) -> vec4<i32> {
    return vec4<i32>((vec4<u32>(p) >> vec4<u32>(0u,8u,16u,24u)) & vec4<u32>(255u));
}

fn paeth(left: vec4<i32>, above: vec4<i32>, corner: vec4<i32>) -> vec4<i32> {
    let a=abs(above-corner); let b=abs(left-corner); let c=abs(left+above-2*corner);
    return select(select(corner,above,b<=c),left,(a<=b)&(a<=c));
}

fn residual_score(px: vec4<i32>, predict: vec4<i32>) -> u32 {
    let diff=(px-predict)&vec4<i32>(255);
    let cost=min(diff,vec4<i32>(256)-diff);
    return u32(cost.x+cost.y+cost.z+cost.w);
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let y=group.x;
    var sums: array<u32,5>;
    for(var x=lane; x<extent.width; x+=64u) {
        let i=y*extent.width+x;
        let p=channels(pixels[i]);
        var a=vec4<i32>(0); var b=vec4<i32>(0); var c=vec4<i32>(0);
        if x>0u { a=channels(pixels[i-1u]); }
        if y>0u {
            b=channels(pixels[i-extent.width]);
            if x>0u { c=channels(pixels[i-extent.width-1u]); }
        }
        sums[0]+=residual_score(p,vec4<i32>(0));
        sums[1]+=residual_score(p,a); sums[2]+=residual_score(p,b);
        sums[3]+=residual_score(p,(a+b)/vec4<i32>(2));
        sums[4]+=residual_score(p,paeth(a,b,c));
    }
    for(var k=0u; k<5u; k++) { partial[lane][k]=sums[k]; }
    workgroupBarrier();
    for(var split=32u; split>0u; split=split/2u) {
        if lane<split {
            for(var k=0u; k<5u; k++) { partial[lane][k]+=partial[lane+split][k]; }
        }
        workgroupBarrier();
    }
    if lane==0u {
        var best=0u;
        for(var k=1u; k<5u; k++) { if partial[0][k]<partial[0][best] { best=k; } }
        selected[y]=best;
    }
}
