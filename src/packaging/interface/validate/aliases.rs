use super::super::*;

pub(super) fn validate_alias_contracts(
    interface: &Interface,
    bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    for binding in &interface.bindings {
        let expected = metadata_alias_spellings(&binding.metadata, &binding.canonical)
            .into_iter()
            .map(|(spelling, role)| {
                (
                    spelling,
                    match role {
                        MetadataAliasRole::Preferred => PublicAliasRole::Preferred,
                        MetadataAliasRole::Migration => PublicAliasRole::Migration,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (spelling, role) in &expected {
            if !interface.aliases.iter().any(|alias| {
                alias.target == binding.id && alias.canonical == *spelling && alias.role == *role
            }) {
                return Err(alias_contract_error(format!(
                    "binding `{}` metadata name `{spelling}` is missing its `{}` public alias role",
                    binding.canonical,
                    role_name(*role),
                )));
            }
        }
    }

    for alias in &interface.aliases {
        if alias.role != PublicAliasRole::Preferred {
            continue;
        }
        let target = bindings
            .get(alias.target.as_str())
            .expect("alias targets were validated before alias contracts");
        let role = metadata_alias_spellings(&target.metadata, &target.canonical)
            .into_iter()
            .find_map(|(spelling, role)| (spelling == alias.canonical).then_some(role));
        if role != Some(MetadataAliasRole::Preferred) {
            return Err(alias_contract_error(format!(
                "preferred alias `{}` is not declared by binding `{}` metadata",
                alias.canonical, target.canonical,
            )));
        }
    }

    for function in &interface.functions {
        for parameter in &function.parameters {
            validate_local_aliases(
                "parameter",
                &parameter.canonical,
                &parameter.aliases,
                &parameter.metadata,
            )?;
        }
    }
    for structure in &interface.structs {
        for field in &structure.fields {
            validate_local_aliases("field", &field.canonical, &field.aliases, &field.metadata)?;
        }
    }
    Ok(())
}

fn validate_local_aliases(
    kind: &str,
    canonical: &str,
    aliases: &[String],
    metadata: &[MetadataEntry],
) -> InterfaceResult<()> {
    let actual = aliases.iter().cloned().collect::<BTreeSet<_>>();
    let expected = metadata_aliases(metadata, canonical)
        .into_iter()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(alias_contract_error(format!(
            "{kind} `{canonical}` alias table does not match its `:osiris/names` metadata"
        )));
    }
    Ok(())
}

const fn role_name(role: PublicAliasRole) -> &'static str {
    match role {
        PublicAliasRole::Preferred => "preferred",
        PublicAliasRole::Migration => "migration",
    }
}

fn alias_contract_error(message: String) -> InterfaceError {
    InterfaceError::new("OSR-I0086", message)
}
