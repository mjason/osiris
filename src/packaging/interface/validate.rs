use super::*;

mod metadata;
mod model;
mod nominal;
mod operators;

pub(in crate::interface) use metadata::{
    metadata_resource_error, validate_interface_metadata_resources, validate_metadata_target,
};
pub(in crate::interface) use model::validate_model;

pub(in crate::interface) fn validate(interface: &Interface) -> InterfaceResult<()> {
    if interface.format_version != FORMAT_VERSION {
        return Err(InterfaceError::new(
            "OSR-I0012",
            format!("unsupported format version `{}`", interface.format_version),
        ));
    }
    if interface.compiler_abi != COMPILER_ABI {
        return Err(InterfaceError::new(
            "OSR-I0013",
            format!("incompatible compiler ABI `{}`", interface.compiler_abi),
        ));
    }
    if interface.language_abi != LANGUAGE_ABI {
        return Err(InterfaceError::new(
            "OSR-I0014",
            format!("incompatible language ABI `{}`", interface.language_abi),
        ));
    }
    validate_model(interface)?;
    verify_interface_hash_group(&interface.graph)
        .map_err(|error| InterfaceError::new("OSR-I0073", error.to_string()))?;
    let expected = calculate_hashes(interface);
    let member = interface
        .graph
        .members
        .iter()
        .find(|member| member.module == interface.module)
        .ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0073",
                format!(
                    "interface graph group does not contain module `{}`",
                    interface.module
                ),
            )
        })?;
    if member.semantic_body_hash != expected.semantic_body
        || member.tooling_body_hash != expected.tooling_body
    {
        return Err(InterfaceError::new(
            "OSR-I0073",
            "interface graph member body hashes do not match the interface body",
        ));
    }
    if interface.hashes != expected {
        return Err(InterfaceError::new(
            "OSR-I0015",
            "interface hash validation failed",
        ));
    }
    Ok(())
}
