//! Output file names from the user's template (SPEC §15). Supports `{date}` and `{time}`;
//! anything else is kept verbatim. Pure: no file-system access here.

pub use chrono::NaiveDateTime;

/// Local wall-clock time, which is what the file name shows.
pub fn now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

pub const DEFAULT_TEMPLATE: &str = "Reshot_{date}_{time}";

pub fn build(template: &str, extension: &str, now: NaiveDateTime) -> String {
    let template = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    let name = template
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M-%S").to_string());
    let ext = extension.trim_start_matches('.').to_lowercase();
    format!("{name}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 28).unwrap().and_hms_opt(9, 5, 3).unwrap()
    }

    #[test]
    fn expands_placeholders() {
        assert_eq!(build("Reshot_{date}_{time}", ".PNG", at()), "Reshot_2026-07-28_09-05-03.png");
    }

    #[test]
    fn empty_template_falls_back_to_default() {
        assert_eq!(build("  ", "jpg", at()), "Reshot_2026-07-28_09-05-03.jpg");
    }
}
