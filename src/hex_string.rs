use std::borrow::Cow;

pub(crate) fn decode(
    value: &str,
    reverse: bool,
    ignore_whitespace: bool,
) -> Result<Vec<u8>, &'static str> {
    let value: Cow<'_, str> = if ignore_whitespace && value.contains(char::is_whitespace) {
        value
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .into()
    } else {
        value.into()
    };
    let invalid = "expected complete hexadecimal bytes";
    if !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err(invalid);
    }
    let mut bytes = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| invalid))
        .collect::<Result<Vec<_>, _>>()?;
    if reverse {
        bytes.reverse();
    }
    Ok(bytes)
}
