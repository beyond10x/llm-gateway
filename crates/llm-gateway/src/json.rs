//! A deliberate JSON writer.
//!
//! The gateway derives no serialisation. Every byte that reaches a client is written by an
//! explicit call here, so a field cannot appear on the wire by being added to a struct, and a
//! credential cannot appear by a type gaining a derive.

/// Writes compact JSON. `fresh` records that the previous byte opened a container or a member,
/// which is the only state a separator decision needs at any nesting depth.
pub(crate) struct Writer {
    out: String,
    fresh: bool,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self {
            out: String::new(),
            fresh: true,
        }
    }

    fn separate(&mut self) {
        if !self.fresh {
            self.out.push(',');
        }
    }

    pub(crate) fn begin_object(&mut self) {
        self.separate();
        self.out.push('{');
        self.fresh = true;
    }

    pub(crate) fn end_object(&mut self) {
        self.out.push('}');
        self.fresh = false;
    }

    pub(crate) fn begin_array(&mut self) {
        self.separate();
        self.out.push('[');
        self.fresh = true;
    }

    pub(crate) fn end_array(&mut self) {
        self.out.push(']');
        self.fresh = false;
    }

    pub(crate) fn key(&mut self, key: &str) {
        self.separate();
        escaped(&mut self.out, key);
        self.out.push(':');
        self.fresh = true;
    }

    pub(crate) fn string(&mut self, value: &str) {
        self.separate();
        escaped(&mut self.out, value);
        self.fresh = false;
    }

    pub(crate) fn number(&mut self, value: u64) {
        self.separate();
        self.out.push_str(itoa(value).as_str());
        self.fresh = false;
    }

    pub(crate) fn boolean(&mut self, value: bool) {
        self.separate();
        self.out.push_str(if value { "true" } else { "false" });
        self.fresh = false;
    }

    pub(crate) fn finish(self) -> String {
        self.out
    }
}

fn itoa(value: u64) -> String {
    value.to_string()
}

fn escaped(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < ' ' || control == '\u{7f}' => {
                out.push_str("\\u");
                let code = control as u32;
                for shift in [12_u32, 8, 4, 0] {
                    let nibble = (code >> shift) & 0xf;
                    out.push(char::from_digit(nibble, 16).unwrap_or('0'));
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::Writer;

    #[test]
    fn nested_containers_separate_their_members_at_every_depth() {
        let mut writer = Writer::new();
        writer.begin_object();
        writer.key("routes");
        writer.begin_array();
        writer.begin_object();
        writer.key("alias");
        writer.string("code");
        writer.key("fallback_enabled");
        writer.boolean(false);
        writer.key("positions");
        writer.begin_array();
        writer.number(0);
        writer.number(1);
        writer.end_array();
        writer.end_object();
        writer.end_array();
        writer.key("count");
        writer.number(1);
        writer.end_object();
        assert_eq!(
            writer.finish(),
            "{\"routes\":[{\"alias\":\"code\",\"fallback_enabled\":false,\
             \"positions\":[0,1]}],\"count\":1}"
        );
    }

    #[test]
    fn quotes_backslashes_and_control_bytes_cannot_break_out_of_a_string() {
        let mut writer = Writer::new();
        writer.string("a\"b\\c\nd\u{1}e\u{7f}");
        assert_eq!(writer.finish(), "\"a\\\"b\\\\c\\nd\\u0001e\\u007f\"");
    }
}
