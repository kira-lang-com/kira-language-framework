use crate::DebugLocation;
use crate::parse::{ParseError, error};

pub(crate) fn parse_debug_location(
    line: &str,
    offset: usize,
) -> Result<(&str, Option<DebugLocation>), ParseError> {
    let Some((instruction, location)) = line.rsplit_once(" !loc(") else {
        return Ok((line, None));
    };
    let body = location
        .strip_suffix(')')
        .ok_or_else(|| error(offset, "unclosed debug location"))?;
    let body = body
        .strip_prefix('"')
        .ok_or_else(|| error(offset, "debug location file must be quoted"))?;
    let (file, position) = body
        .split_once("\":")
        .ok_or_else(|| error(offset, "invalid debug location"))?;
    let (line_number, column) = position
        .split_once(':')
        .ok_or_else(|| error(offset, "debug location needs line and column"))?;
    Ok((
        instruction.trim_end(),
        Some(DebugLocation {
            file: file.to_owned(),
            line: line_number
                .parse()
                .map_err(|_| error(offset, "invalid debug line"))?,
            column: column
                .parse()
                .map_err(|_| error(offset, "invalid debug column"))?,
        }),
    ))
}
