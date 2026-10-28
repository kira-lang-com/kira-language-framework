//! The spelling of each built-in on every target (`KLF-KSL.md` Appendix C).

use klf_shader::{BuiltIn, Target};
use std::collections::BTreeMap;

/// The targets, in the order [`TABLE`] lists them: every target that spells
/// a built-in at all.
const TARGETS: [Target; 6] = [
    Target::Msl,
    Target::Wgsl,
    Target::Glsl,
    Target::Hlsl,
    Target::Spirv,
    Target::Ksl,
];

/// One row per built-in in `BuiltIn::ALL` order, one entry per [`TARGETS`]
/// entry; `None` is a target with no spelling for it. MSL has no workgroup
/// id and no clip distances: both need a capability the standard leaves to
/// the shader.
const TABLE: [[Option<&'static str>; 6]; 13] = [
    // Position
    [
        Some("position"),
        Some("@builtin(position)"),
        Some("gl_Position"),
        Some("SV_Position"),
        Some("PositionBuiltIn"),
        Some("@builtin(position)"),
    ],
    // VertexIndex
    [
        Some("vertex_id"),
        Some("@builtin(vertex_index)"),
        Some("gl_VertexID"),
        Some("SV_VertexID"),
        Some("VertexIndexBuiltIn"),
        Some("@builtin(vertex_index)"),
    ],
    // InstanceIndex
    [
        Some("instance_id"),
        Some("@builtin(instance_index)"),
        Some("gl_InstanceID"),
        Some("SV_InstanceID"),
        Some("InstanceIndexBuiltIn"),
        Some("@builtin(instance_index)"),
    ],
    // FrontFacing
    [
        Some("front_facing"),
        Some("@builtin(front_facing)"),
        Some("gl_FrontFacing"),
        Some("SV_IsFrontFace"),
        Some("FrontFacingBuiltIn"),
        Some("@builtin(front_facing)"),
    ],
    // FragDepth
    [
        Some("depth(any)"),
        Some("@builtin(frag_depth)"),
        Some("gl_FragDepth"),
        Some("SV_Depth"),
        Some("FragDepthBuiltIn"),
        Some("@builtin(frag_depth)"),
    ],
    // SampleIndex
    [
        Some("sample_id"),
        Some("@builtin(sample_index)"),
        Some("gl_SampleID"),
        Some("SV_SampleIndex"),
        Some("SampleIndexBuiltIn"),
        Some("@builtin(sample_index)"),
    ],
    // SampleMask
    [
        Some("sample_mask"),
        Some("@builtin(sample_mask)"),
        Some("gl_SampleMask"),
        Some("SV_Coverage"),
        Some("SampleMaskBuiltIn"),
        Some("@builtin(sample_mask)"),
    ],
    // GlobalInvocationId
    [
        Some("threadgroup_position_in_grid"),
        Some("@builtin(global_invocation_id)"),
        Some("gl_GlobalInvocationID"),
        Some("SV_DispatchThreadID"),
        Some("GlobalInvocationIdBuiltIn"),
        Some("@builtin(global_invocation_id)"),
    ],
    // LocalInvocationId
    [
        Some("thread_position_in_threadgroup"),
        Some("@builtin(local_invocation_id)"),
        Some("gl_LocalInvocationID"),
        Some("SV_GroupThreadID"),
        Some("LocalInvocationIdBuiltIn"),
        Some("@builtin(local_invocation_id)"),
    ],
    // LocalInvocationIndex
    [
        Some("thread_index_in_threadgroup"),
        Some("@builtin(local_invocation_index)"),
        Some("gl_LocalInvocationIndex"),
        Some("SV_GroupIndex"),
        Some("LocalInvocationIndexBuiltIn"),
        Some("@builtin(local_invocation_index)"),
    ],
    // WorkgroupId
    [
        None,
        Some("@builtin(workgroup_id)"),
        Some("gl_WorkGroupID"),
        Some("SV_GroupID"),
        Some("WorkgroupIdBuiltIn"),
        Some("@builtin(workgroup_id)"),
    ],
    // NumWorkgroups
    [
        Some("threadgroups_per_grid"),
        Some("@builtin(num_workgroups)"),
        Some("gl_NumWorkGroups"),
        Some("SV_DispatchGroupID"),
        Some("NumWorkgroupsBuiltIn"),
        Some("@builtin(num_workgroups)"),
    ],
    // ClipDistances
    [
        None,
        Some("ref(clip_distances)"),
        Some("gl_ClipDistance"),
        Some("SV_ClipDistance"),
        Some("ClipDistanceBuiltIn"),
        Some("@builtin(clip_distances)"),
    ],
];

/// The spelling of `builtin` on each target, `None` where the target has
/// none.
#[must_use]
pub fn spellings(builtin: BuiltIn) -> BTreeMap<Target, Option<&'static str>> {
    let row = BuiltIn::ALL
        .iter()
        .position(|known| *known == builtin)
        .map_or(&TABLE[0], |index| &TABLE[index]);
    TARGETS.iter().copied().zip(row.iter().copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::{TABLE, TARGETS};

    /// The table is in `BuiltIn::ALL` order, so a row that moved would spell
    /// one built-in as another.
    #[test]
    fn every_builtin_has_a_row() {
        assert_eq!(TABLE.len(), klf_shader::BuiltIn::ALL.len());
        assert_eq!(TARGETS.len(), TABLE[0].len());
    }
}
