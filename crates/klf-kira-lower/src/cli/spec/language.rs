use super::*;

/// The Language's own command line: its `configuration` and the options its
/// `@Flag` and `@Option` members declare. Built-in commands are the kernel's
/// and are added by the driver.
pub(crate) fn language_command(
    session: &mut Session,
    file: &File,
    members: &[&Member],
    driver: &str,
) -> Option<(Command, Vec<klf_services::DriverFlag>)> {
    let decls = commands(session, file)?;
    let by_name: HashMap<Name, &CommandDecl> =
        decls.iter().map(|decl| (decl.name.clone(), decl)).collect();
    let mut root = Command::new(driver);
    let mut config = Config::default();
    let mut flags = Vec::new();
    for member in members {
        let MemberKind::Field { name, ty, init, .. } = &member.kind else {
            continue;
        };
        if name.name.as_str() == "configuration" {
            let Some(FieldInit::Expr(value)) = init else {
                report(
                    session,
                    member.span,
                    "`configuration` needs a CommandConfiguration literal",
                );
                return None;
            };
            config = configuration(session, value)?;
            continue;
        }
        let Some(annotation) = member
            .annotations
            .iter()
            .find(|annotation| matches!(annotation_name(&annotation.path), Some(OPTION | FLAG)))
        else {
            continue;
        };
        let init = match init {
            Some(FieldInit::Expr(value)) => Some(value),
            _ => None,
        };
        let (arg, scalar, shape) = argument(session, annotation, name, ty, init, &HashMap::new())?;
        let default = init.map(|init| (&init.kind, &scalar));
        let value = match default {
            Some((ExprKind::Bool(value), Scalar::Bool)) if shape == Shape::Single => {
                klf_services::DriverFlagValue::Boolean(*value)
            }
            Some((ExprKind::Str(text), Scalar::String)) if shape == Shape::Single => {
                klf_services::DriverFlagValue::String(text.clone())
            }
            // A repeatable option, `-DNAME -DOTHER`: the definition reads its
            // values as one String, one value per line.
            Some((ExprKind::ArrayLit(items), Scalar::String))
                if shape == Shape::List && items.is_empty() =>
            {
                klf_services::DriverFlagValue::String(String::new())
            }
            Some((_, Scalar::Int)) if shape == Shape::Single => {
                let Some(text) = default_text(init?, &scalar) else {
                    report(
                        session,
                        member.span,
                        "a Language option's initial value is a literal",
                    );
                    return None;
                };
                klf_services::DriverFlagValue::Integer(text.parse().ok()?)
            }
            _ => {
                report(
                    session,
                    member.span,
                    "a Language option is a Bool, Int or String with a literal initial value, or a [String] starting empty",
                );
                return None;
            }
        };
        let Some(long) = arg.long.clone() else {
            report(session, member.span, "a Language option needs a long name");
            return None;
        };
        let mut arg = arg;
        arg.id.clone_from(&long);
        arg.required = false;
        root.args.push(arg);
        flags.push(klf_services::DriverFlag {
            name: long,
            default_value: value,
        });
    }
    root.abstract_text = config.abstract_text;
    root.discussion = config.discussion;
    let (subcommands, _) = resolve(
        session,
        &config.subcommands,
        None,
        &by_name,
        &mut Vec::new(),
    )?;
    root.subcommands = subcommands;
    Some((root, flags))
}
