use soulseek::client::ProfileRequest;

use crate::transfers::TransferTotalsSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueVar {
    UserName,
    UserRestriction,
    UserQueuedFiles,
    UserActiveUploads,
    MyName,
    MySharedFiles,
    MySharedFolders,
    MyQueueSize,
    MySlots,
    MyFreeSlots,
    MyUploadSpeed,
    StatsUploads,
    StatsUploaded,
    StatsUploadUsers,
    StatsDownloads,
    StatsDownloaded,
    StatsUploadSpeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagVar {
    Buddy,
    Ignored,
    Banned,
    Privileged,
}

const VALUE_VARS: [(&str, ValueVar); 17] = [
    ("user.name", ValueVar::UserName),
    ("user.restriction", ValueVar::UserRestriction),
    ("user.queued_files", ValueVar::UserQueuedFiles),
    ("user.active_uploads", ValueVar::UserActiveUploads),
    ("me.name", ValueVar::MyName),
    ("me.shared_files", ValueVar::MySharedFiles),
    ("me.shared_folders", ValueVar::MySharedFolders),
    ("me.queue_size", ValueVar::MyQueueSize),
    ("me.slots", ValueVar::MySlots),
    ("me.free_slots", ValueVar::MyFreeSlots),
    ("me.upload_speed", ValueVar::MyUploadSpeed),
    ("stats.uploads", ValueVar::StatsUploads),
    ("stats.uploaded", ValueVar::StatsUploaded),
    ("stats.upload_users", ValueVar::StatsUploadUsers),
    ("stats.downloads", ValueVar::StatsDownloads),
    ("stats.downloaded", ValueVar::StatsDownloaded),
    ("stats.upload_speed", ValueVar::StatsUploadSpeed),
];

const FLAG_VARS: [(&str, FlagVar); 4] = [
    ("user.is_buddy", FlagVar::Buddy),
    ("user.is_ignored", FlagVar::Ignored),
    ("user.is_banned", FlagVar::Banned),
    ("user.is_privileged", FlagVar::Privileged),
];

fn lookup<T: Copy>(vars: &[(&str, T)], name: &str) -> Option<T> {
    vars.iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, var)| *var)
}

fn names<T>(vars: &[(&'static str, T)]) -> Vec<&'static str> {
    vars.iter().map(|(name, _)| *name).collect()
}

#[cfg(test)]
pub fn description_variables() -> Vec<&'static str> {
    [names(&VALUE_VARS), names(&FLAG_VARS)].concat()
}

fn unknown_variable(name: &str) -> String {
    format!(
        "unknown description variable {name:?}: values are {}; yes/no flags are {}",
        names(&VALUE_VARS).join(", "),
        names(&FLAG_VARS).join(", ")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Text(String),
    Value(ValueVar),
    Flag {
        var: FlagVar,
        then: String,
        otherwise: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DescriptionTemplate(Vec<Segment>);

impl DescriptionTemplate {
    pub fn parse(template: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut text = String::new();
        let mut rest = template;
        while let Some(at) = rest.find('$') {
            text.push_str(&rest[..at]);
            rest = &rest[at + 1..];
            if let Some(after) = rest.strip_prefix('$') {
                text.push('$');
                rest = after;
                continue;
            }
            let Some(after) = rest.strip_prefix('{') else {
                text.push('$');
                continue;
            };
            let Some(end) = after.find('}') else {
                return Err(
                    "unterminated ${ in description: write $$ for a literal dollar sign".to_owned(),
                );
            };
            let segment = parse_segment(&after[..end])?;
            rest = &after[end + 1..];
            if !text.is_empty() {
                segments.push(Segment::Text(std::mem::take(&mut text)));
            }
            segments.push(segment);
        }
        text.push_str(rest);
        if !text.is_empty() {
            segments.push(Segment::Text(text));
        }
        Ok(Self(segments))
    }

    pub fn render(&self, request: &ProfileRequest<'_>, totals: &TransferTotalsSnapshot) -> String {
        let mut out = String::new();
        for segment in &self.0 {
            match segment {
                Segment::Text(text) => out.push_str(text),
                Segment::Value(var) => value(*var, request, totals, &mut out),
                Segment::Flag {
                    var,
                    then,
                    otherwise,
                } => out.push_str(if flag(*var, request) { then } else { otherwise }),
            }
        }
        out
    }
}

fn parse_segment(body: &str) -> Result<Segment, String> {
    match body.split_once('?') {
        Some((name, arms)) => {
            let Some(var) = lookup(&FLAG_VARS, name) else {
                return Err(if lookup(&VALUE_VARS, name).is_some() {
                    format!(
                        "description variable {name} holds a value, not a yes/no flag: \
                         write ${{{name}}}"
                    )
                } else {
                    unknown_variable(name)
                });
            };
            let Some((then, otherwise)) = arms.split_once(':') else {
                return Err(format!(
                    "description flag {name} needs two texts separated by a colon: \
                     write ${{{name}?yes:no}}"
                ));
            };
            Ok(Segment::Flag {
                var,
                then: then.replace("$$", "$"),
                otherwise: otherwise.replace("$$", "$"),
            })
        }
        None => {
            let Some(var) = lookup(&VALUE_VARS, body) else {
                return Err(if lookup(&FLAG_VARS, body).is_some() {
                    format!(
                        "description variable {body} is a yes/no flag: \
                         write ${{{body}?yes:no}} to pick the text for each case"
                    )
                } else {
                    unknown_variable(body)
                });
            };
            Ok(Segment::Value(var))
        }
    }
}

fn value(
    var: ValueVar,
    request: &ProfileRequest<'_>,
    totals: &TransferTotalsSnapshot,
    out: &mut String,
) {
    match var {
        ValueVar::UserName => out.push_str(request.user_name),
        ValueVar::UserRestriction => out.push_str(request.user_restriction),
        ValueVar::MyName => out.push_str(request.my_name),
        ValueVar::UserQueuedFiles => out.push_str(&request.user_queued_files.to_string()),
        ValueVar::UserActiveUploads => out.push_str(&request.user_active_uploads.to_string()),
        ValueVar::MySharedFiles => out.push_str(&request.my_shared_files.to_string()),
        ValueVar::MySharedFolders => out.push_str(&request.my_shared_folders.to_string()),
        ValueVar::MyQueueSize => out.push_str(&request.my_queue_size.to_string()),
        ValueVar::MySlots => out.push_str(&request.my_slots.to_string()),
        ValueVar::MyFreeSlots => out.push_str(&request.my_free_slots.to_string()),
        ValueVar::MyUploadSpeed => {
            out.push_str(&format_bytes(request.my_upload_speed.into(), true))
        }
        ValueVar::StatsUploads => out.push_str(&totals.uploads.count.to_string()),
        ValueVar::StatsUploaded => out.push_str(&format_bytes(totals.uploads.bytes, false)),
        ValueVar::StatsUploadUsers => out.push_str(&totals.upload_users.to_string()),
        ValueVar::StatsDownloads => out.push_str(&totals.downloads.count.to_string()),
        ValueVar::StatsDownloaded => out.push_str(&format_bytes(totals.downloads.bytes, false)),
        ValueVar::StatsUploadSpeed => {
            out.push_str(&format_bytes(totals.uploads.avg_speed_bps, true))
        }
    }
}

fn flag(var: FlagVar, request: &ProfileRequest<'_>) -> bool {
    match var {
        FlagVar::Buddy => request.user_is_buddy,
        FlagVar::Ignored => request.user_is_ignored,
        FlagVar::Banned => request.user_ban_reason.is_some(),
        FlagVar::Privileged => request.user_is_privileged,
    }
}

fn format_bytes(bytes: u64, per_second: bool) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let suffix = if per_second { "/s" } else { "" };
    if bytes < 1024 {
        return format!("{bytes} B{suffix}");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}{suffix}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfers::DirectionTotals;

    fn request() -> ProfileRequest<'static> {
        ProfileRequest {
            user_name: "asker",
            user_restriction: "hold",
            user_queued_files: 3,
            user_active_uploads: 1,
            user_is_buddy: true,
            user_is_ignored: false,
            user_ban_reason: None,
            user_is_privileged: false,
            my_name: "me",
            my_shared_files: 12043,
            my_shared_folders: 402,
            my_queue_size: 7,
            my_slots: 2,
            my_free_slots: 1,
            my_upload_speed: 524_288,
        }
    }

    fn totals() -> TransferTotalsSnapshot {
        TransferTotalsSnapshot {
            uploads: DirectionTotals {
                count: 12,
                bytes: 1_539_316_278_886,
                avg_speed_bps: 524_288,
            },
            downloads: DirectionTotals {
                count: 7,
                bytes: 1024,
                avg_speed_bps: 0,
            },
            upload_users: 5,
        }
    }

    fn render(template: &str) -> String {
        DescriptionTemplate::parse(template)
            .unwrap()
            .render(&request(), &totals())
    }

    #[test]
    fn substitutes_values() {
        assert_eq!(
            render(
                "Hi ${user.name}! I share ${me.shared_files} files in ${me.shared_folders} folders."
            ),
            "Hi asker! I share 12043 files in 402 folders."
        );
        assert_eq!(
            render("restriction: ${user.restriction}"),
            "restriction: hold"
        );
    }

    #[test]
    fn transfer_values_render() {
        assert_eq!(
            render(
                "${stats.uploads}|${stats.uploaded}|${stats.upload_users}|${stats.downloads}|${stats.downloaded}|${stats.upload_speed}|${me.upload_speed}"
            ),
            "12|1.4 TiB|5|7|1.0 KiB|512.0 KiB/s|512.0 KiB/s"
        );
    }

    #[test]
    fn byte_boundaries_render() {
        assert_eq!(format_bytes(0, false), "0 B");
        assert_eq!(format_bytes(1023, false), "1023 B");
        assert_eq!(format_bytes(1024, false), "1.0 KiB");
        assert_eq!(format_bytes(1_539_316_278_886, false), "1.4 TiB");
    }

    #[test]
    fn flags_pick_an_arm() {
        assert_eq!(
            render("${user.is_buddy?Hey buddy:Hi} ${user.name}"),
            "Hey buddy asker"
        );
        assert_eq!(
            render("${user.is_privileged?privileged:regular} user"),
            "regular user"
        );
        assert_eq!(render("${user.is_banned?no:}welcome"), "welcome");
    }

    #[test]
    fn dollar_is_literal_unless_it_opens_a_variable() {
        assert_eq!(render("costs $5 and $$ signs"), "costs $5 and $ signs");
        assert_eq!(render("$${user.name}"), "${user.name}");
        assert_eq!(render("trailing $"), "trailing $");
        assert_eq!(render("${user.is_buddy?$$5:$$0}"), "$5");
    }

    #[test]
    fn unknown_variable_is_rejected() {
        let error = DescriptionTemplate::parse("hi ${user.nope}").unwrap_err();
        assert!(error.contains("user.nope"), "{error}");
        assert!(error.contains("user.name"), "{error}");
    }

    #[test]
    fn bare_flag_is_rejected() {
        let error = DescriptionTemplate::parse("${user.is_buddy}").unwrap_err();
        assert!(error.contains("${user.is_buddy?yes:no}"), "{error}");
    }

    #[test]
    fn ternary_on_a_value_is_rejected() {
        let error = DescriptionTemplate::parse("${user.name?a:b}").unwrap_err();
        assert!(error.contains("holds a value"), "{error}");
    }

    #[test]
    fn flag_without_both_arms_is_rejected() {
        let error = DescriptionTemplate::parse("${user.is_buddy?only}").unwrap_err();
        assert!(error.contains("colon"), "{error}");
    }

    #[test]
    fn unterminated_variable_is_rejected() {
        let error = DescriptionTemplate::parse("hi ${user.name").unwrap_err();
        assert!(error.contains("unterminated"), "{error}");
    }

    #[test]
    fn no_name_is_both_a_value_and_a_flag() {
        for (name, _) in VALUE_VARS {
            assert!(lookup(&FLAG_VARS, name).is_none(), "{name}");
        }
    }
}
