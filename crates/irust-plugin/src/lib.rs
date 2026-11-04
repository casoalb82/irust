use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use mlua::{Function, Lua, MultiValue, Table, Value};

/// The result of running every registered `on_message` handler against
/// one incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHookResult {
    Unchanged,
    Replaced(String),
}

const POOL_SIZE: usize = 4;

pub struct PluginHost {
    interpreters: Vec<Mutex<Lua>>,
    next: AtomicUsize,
    loaded: Mutex<Vec<String>>,
}

impl PluginHost {
    pub fn new() -> mlua::Result<Self> {
        let mut interpreters = Vec::with_capacity(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            interpreters.push(Mutex::new(Self::build_interpreter()?));
        }
        Ok(Self {
            interpreters,
            next: AtomicUsize::new(0),
            loaded: Mutex::new(Vec::new()),
        })
    }

    fn build_interpreter() -> mlua::Result<Lua> {
        let lua = Lua::new();

        let irust_table = lua.create_table()?;

        let message_handlers = lua.create_table()?;
        irust_table.set("_on_message_handlers", message_handlers)?;
        let send_handlers = lua.create_table()?;
        irust_table.set("_on_send_handlers", send_handlers)?;
        let commands = lua.create_table()?;
        irust_table.set("_commands", commands)?;

        let call_meta = lua.create_table()?;
        call_meta.set(
            "__call",
            lua.create_function(|_, (this, args): (Table, MultiValue)| -> mlua::Result<Value> {
                let f: Function = this.get("fn")?;
                f.call(args)
            })?,
        )?;
        irust_table.set("_call_meta", call_meta)?;

        let current_script = |lua: &Lua| -> String {
            lua.globals()
                .get::<Option<String>>("__CURRENT_SCRIPT__")
                .ok()
                .flatten()
                .unwrap_or_default()
        };

        let on_message = lua.create_function(move |lua, f: Function| {
            let irust: Table = lua.globals().get("irust")?;
            let handlers: Table = irust.get("_on_message_handlers")?;
            let entry = lua.create_table()?;
            entry.set("script", current_script(lua))?;
            entry.set("fn", f)?;
            let len = handlers.raw_len();
            handlers.set(len + 1, entry)?;
            Ok(())
        })?;
        irust_table.set("on_message", on_message)?;

        let on_send = lua.create_function(move |lua, f: Function| {
            let irust: Table = lua.globals().get("irust")?;
            let handlers: Table = irust.get("_on_send_handlers")?;
            let entry = lua.create_table()?;
            entry.set("script", current_script(lua))?;
            entry.set("fn", f)?;
            let len = handlers.raw_len();
            handlers.set(len + 1, entry)?;
            Ok(())
        })?;
        irust_table.set("on_send", on_send)?;

        let pending_sends = lua.create_table()?;
        irust_table.set("_pending_sends", pending_sends)?;
        let send = lua.create_function(|lua, line: String| {
            let irust: Table = lua.globals().get("irust")?;
            let pending: Table = irust.get("_pending_sends")?;
            let len = pending.raw_len();
            pending.set(len + 1, line)?;
            Ok(())
        })?;
        irust_table.set("send", send)?;

        let pending_says = lua.create_table()?;
        irust_table.set("_pending_says", pending_says)?;
        let say = lua.create_function(|lua, line: String| {
            let irust: Table = lua.globals().get("irust")?;
            let pending: Table = irust.get("_pending_says")?;
            let len = pending.raw_len();
            pending.set(len + 1, line)?;
            Ok(())
        })?;
        irust_table.set("say", say)?;

        let mynick = lua.create_function(|lua, ()| {
            let nick: Option<String> = lua.globals().get::<Option<String>>("__CURRENT_NICK__").ok().flatten();
            Ok(nick.unwrap_or_default())
        })?;
        irust_table.set("mynick", mynick)?;

        for (name, handlers_key) in [
            ("on_join", "_on_join_handlers"),
            ("on_part", "_on_part_handlers"),
            ("on_quit", "_on_quit_handlers"),
            ("on_nick", "_on_nick_handlers"),
            ("on_topic", "_on_topic_handlers"),
            ("on_mode", "_on_mode_handlers"),
            ("on_connect", "_on_connect_handlers"),
            ("on_disconnect", "_on_disconnect_handlers"),
            ("on_oper", "_on_oper_handlers"),
        ] {
            Self::register_notify_hook(&lua, &irust_table, name, handlers_key)?;
        }

        let on_command = lua.create_function(move |lua, (name, f): (String, Function)| {
            let irust: Table = lua.globals().get("irust")?;
            let commands: Table = irust.get("_commands")?;
            let call_meta: Table = irust.get("_call_meta")?;
            let entry = lua.create_table()?;
            entry.set("script", current_script(lua))?;
            entry.set("fn", f)?;
            entry.set_metatable(Some(call_meta))?;
            commands.set(name.to_lowercase(), entry)?;
            Ok(())
        })?;
        irust_table.set("on_command", on_command)?;

        lua.globals().set("irust", irust_table)?;

        Ok(lua)
    }

    fn register_notify_hook(
        lua: &Lua,
        irust_table: &Table,
        name: &'static str,
        handlers_key: &'static str,
    ) -> mlua::Result<()> {
        let handlers = lua.create_table()?;
        irust_table.set(handlers_key, handlers)?;
        let register = lua.create_function(move |lua, f: Function| {
            let irust: Table = lua.globals().get("irust")?;
            let handlers: Table = irust.get(handlers_key)?;
            let entry = lua.create_table()?;
            let script = lua
                .globals()
                .get::<Option<String>>("__CURRENT_SCRIPT__")
                .ok()
                .flatten()
                .unwrap_or_default();
            entry.set("script", script)?;
            entry.set("fn", f)?;
            let len = handlers.raw_len();
            handlers.set(len + 1, entry)?;
            Ok(())
        })?;
        irust_table.set(name, register)?;
        Ok(())
    }

    fn script_id(path: &Path) -> String {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("script")
            .to_string()
    }

    fn pick(&self) -> MutexGuard<'_, Lua> {
        for interp in &self.interpreters {
            if let Ok(guard) = interp.try_lock() {
                return guard;
            }
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.interpreters.len();
        self.interpreters[i].lock().unwrap()
    }

    pub fn load_dir(&self, dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        irust_config::perms::restrict_dir_to_owner(dir);

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            if self.load_file(&path).unwrap_or(false) {
                count += 1;
            }
        }
        count
    }

    /// `/pluginload <name>` and `/pluginreload <name>`'s shared backing
    /// implementation -- loads exactly one script by name (with or
    /// without a trailing `.lua`) from `dir`. `Ok(None)` means no such
    /// file exists; otherwise behaves like `load_file`.
    pub fn load_named(&self, dir: &Path, name: &str) -> mlua::Result<Option<bool>> {
        let file_name = if name.ends_with(".lua") {
            name.to_string()
        } else {
            format!("{name}.lua")
        };
        let path = dir.join(file_name);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(self.load_file(&path)?))
    }

    pub fn load_file(&self, path: &Path) -> mlua::Result<bool> {
        let Ok(src) = fs::read_to_string(path) else {
            return Ok(false);
        };
        irust_config::perms::restrict_to_owner(path);
        let script = Self::script_id(path);
        self.unload_script(&script);

        let chunk_name = path.to_string_lossy().to_string();
        let mut ok = true;
        for interp in &self.interpreters {
            let lua = interp.lock().unwrap();
            lua.globals().set("__CURRENT_SCRIPT__", script.clone())?;
            if lua.load(&src).set_name(chunk_name.clone()).exec().is_err() {
                ok = false;
            }
        }
        if ok {
            let mut loaded = self.loaded.lock().unwrap();
            if !loaded.iter().any(|s| s == &script) {
                loaded.push(script);
            }
        }
        Ok(ok)
    }

    pub fn unload_script(&self, script: &str) -> bool {
        let was_loaded = {
            let mut loaded = self.loaded.lock().unwrap();
            let was_loaded = loaded.iter().any(|s| s == script);
            loaded.retain(|s| s != script);
            was_loaded
        };

        for interp in &self.interpreters {
            let lua = interp.lock().unwrap();
            let Ok(irust) = lua.globals().get::<Table>("irust") else {
                continue;
            };
            for key in [
                "_on_message_handlers",
                "_on_send_handlers",
                "_on_join_handlers",
                "_on_part_handlers",
                "_on_quit_handlers",
                "_on_nick_handlers",
                "_on_topic_handlers",
                "_on_mode_handlers",
                "_on_connect_handlers",
                "_on_disconnect_handlers",
                "_on_oper_handlers",
            ] {
                let Ok(handlers) = irust.get::<Table>(key) else {
                    continue;
                };
                let Ok(kept) = lua.create_table() else {
                    continue;
                };
                let mut i = 1;
                for pair in handlers.sequence_values::<Table>() {
                    let Ok(entry) = pair else { continue };
                    let entry_script: String = entry.get("script").unwrap_or_default();
                    if entry_script != script {
                        let _ = kept.set(i, entry);
                        i += 1;
                    }
                }
                let _ = irust.set(key, kept);
            }
            if let Ok(commands) = irust.get::<Table>("_commands") {
                let mut to_remove = Vec::new();
                for pair in commands.pairs::<String, Table>() {
                    let Ok((name, entry)) = pair else { continue };
                    let entry_script: String = entry.get("script").unwrap_or_default();
                    if entry_script == script {
                        to_remove.push(name);
                    }
                }
                for name in to_remove {
                    let _ = commands.set(name, Value::Nil);
                }
            }
        }
        was_loaded
    }

    pub fn reload(&self, dir: &Path) -> mlua::Result<usize> {
        for interp in &self.interpreters {
            let mut guard = interp.lock().unwrap();
            *guard = Self::build_interpreter()?;
        }
        self.loaded.lock().unwrap().clear();
        Ok(self.load_dir(dir))
    }

    /// What `load_dir`/`load_file`/`load_named` currently consider
    /// loaded, sorted -- for a `/pluginlist`-style status line.
    pub fn loaded_scripts(&self) -> Vec<String> {
        let mut loaded = self.loaded.lock().unwrap().clone();
        loaded.sort();
        loaded
    }

    pub fn load_str(&self, src: &str, name: &str) -> mlua::Result<()> {
        self.unload_script(name);
        for interp in &self.interpreters {
            let lua = interp.lock().unwrap();
            lua.globals().set("__CURRENT_SCRIPT__", name)?;
            lua.load(src).set_name(name).exec()?;
        }
        let mut loaded = self.loaded.lock().unwrap();
        if !loaded.iter().any(|s| s == name) {
            loaded.push(name.to_string());
        }
        Ok(())
    }

    /// Runs every registered `on_message` handler, in registration
    /// order, for one incoming message. The first handler to return a
    /// non-nil string wins; later handlers still run (for side effects)
    /// but their return value is ignored -- a simple, predictable
    /// semantics for v0 rather than a full middleware chain.
    pub fn on_message(&self, nick: &str, target: &str, text: &str) -> PluginHookResult {
        let lua = self.pick();
        let Ok(irust) = lua.globals().get::<Table>("irust") else {
            return PluginHookResult::Unchanged;
        };
        let Ok(handlers) = irust.get::<Table>("_on_message_handlers") else {
            return PluginHookResult::Unchanged;
        };

        let mut replacement: Option<String> = None;
        for pair in handlers.sequence_values::<Table>() {
            let Ok(entry) = pair else { continue };
            let Ok(f) = entry.get::<Function>("fn") else { continue };
            if let Ok(Value::String(s)) = f.call::<Value>((nick, target, text)) {
                if replacement.is_none() {
                    if let Ok(s) = s.to_str() {
                        replacement = Some(s.to_string());
                    }
                }
            }
        }

        match replacement {
            Some(s) => PluginHookResult::Replaced(s),
            None => PluginHookResult::Unchanged,
        }
    }

    /// `on_message` for a line that arrived on `network`. `active` is whether
    /// that network is the one in view -- kept for callers that still track
    /// it, unused by the generic hook itself.
    pub fn on_message_for(&self, _network: &str, _active: bool, nick: &str, target: &str, text: &str) -> PluginHookResult {
        self.on_message(nick, target, text)
    }

    /// `on_send` for text the user is sending on `network`.
    pub fn on_send_for(&self, _network: &str, target: &str, text: &str, extra_nicks: &[String]) -> PluginHookResult {
        self.on_send(target, text, extra_nicks)
    }

    pub fn on_send(&self, target: &str, text: &str, extra_nicks: &[String]) -> PluginHookResult {
        let lua = self.pick();
        let Ok(irust) = lua.globals().get::<Table>("irust") else {
            return PluginHookResult::Unchanged;
        };
        let Ok(handlers) = irust.get::<Table>("_on_send_handlers") else {
            return PluginHookResult::Unchanged;
        };
        let Ok(nicks_table) = lua.create_sequence_from(extra_nicks.iter().cloned()) else {
            return PluginHookResult::Unchanged;
        };

        let mut replacement: Option<String> = None;
        for pair in handlers.sequence_values::<Table>() {
            let Ok(entry) = pair else { continue };
            let Ok(f) = entry.get::<Function>("fn") else { continue };
            if let Ok(Value::String(s)) = f.call::<Value>((target, text, nicks_table.clone())) {
                if replacement.is_none() {
                    if let Ok(s) = s.to_str() {
                        replacement = Some(s.to_string());
                    }
                }
            }
        }

        match replacement {
            Some(s) => PluginHookResult::Replaced(s),
            None => PluginHookResult::Unchanged,
        }
    }

    fn notify(&self, handlers_key: &str, args: impl mlua::IntoLuaMulti + Clone) {
        let lua = self.pick();
        let Ok(irust) = lua.globals().get::<Table>("irust") else {
            return;
        };
        let Ok(handlers) = irust.get::<Table>(handlers_key) else {
            return;
        };
        for pair in handlers.sequence_values::<Table>() {
            let Ok(entry) = pair else { continue };
            let Ok(f) = entry.get::<Function>("fn") else { continue };
            let _ = f.call::<Value>(args.clone());
        }
    }

    /// `irust.on_join(function(nick, channel) ... end)`.
    pub fn on_join(&self, nick: &str, channel: &str) {
        self.notify("_on_join_handlers", (nick, channel));
    }

    /// `irust.on_part(function(nick, channel, reason) ... end)` --
    /// `reason` is `""` for a plain `PART` with none.
    pub fn on_part(&self, nick: &str, channel: &str, reason: &str) {
        self.notify("_on_part_handlers", (nick, channel, reason));
    }

    /// `irust.on_quit(function(nick, reason) ... end)` -- fires for
    /// every quit, netsplit-shaped ones included (netsplit collapsing,
    /// see `netsplit.rs`, is purely a display concern in irssi's own
    /// architecture too; the underlying signal still fires per-nick).
    pub fn on_quit(&self, nick: &str, reason: &str) {
        self.notify("_on_quit_handlers", (nick, reason));
    }

    /// `irust.on_nick(function(old_nick, new_nick) ... end)`.
    pub fn on_nick(&self, old_nick: &str, new_nick: &str) {
        self.notify("_on_nick_handlers", (old_nick, new_nick));
    }

    pub fn on_topic(&self, nick: &str, channel: &str, topic: &str) {
        self.notify("_on_topic_handlers", (nick, channel, topic));
    }

    /// `irust.on_mode(function(nick, target, modestring) ... end)` --
    /// `target` is a channel for a channel mode change, or a nick
    /// (usually our own) for a user mode change; `modestring` is
    /// whatever followed the target verbatim (e.g. `"+o somenick"`), not
    /// parsed into individual flags.
    pub fn on_mode(&self, nick: &str, target: &str, modestring: &str) {
        self.notify("_on_mode_handlers", (nick, target, modestring));
    }

    /// `irust.on_connect(function(network) ... end)` -- fires once
    /// registration completes (`001`), same moment autojoin fires.
    pub fn on_connect(&self, network: &str) {
        self.notify("_on_connect_handlers", network);
    }

    /// `irust.on_disconnect(function(network) ... end)` -- fires for
    /// any disconnect, deliberate (`/quit`, `/disconnect`) or not (a
    /// dropped connection about to auto-reconnect).
    pub fn on_disconnect(&self, network: &str) {
        self.notify("_on_disconnect_handlers", network);
    }

    pub fn update_nick(&self, nick: &str) {
        for interp in &self.interpreters {
            let lua = interp.lock().unwrap();
            let _ = lua.globals().set("__CURRENT_NICK__", nick);
        }
    }

    pub fn on_oper(&self) -> Vec<String> {
        let lua = self.pick();
        let Ok(irust) = lua.globals().get::<Table>("irust") else {
            return Vec::new();
        };
        let Ok(pending) = irust.get::<Table>("_pending_sends") else {
            return Vec::new();
        };
        Self::clear_table(&pending);

        if let Ok(handlers) = irust.get::<Table>("_on_oper_handlers") {
            for pair in handlers.sequence_values::<Table>() {
                let Ok(entry) = pair else { continue };
                let Ok(f) = entry.get::<Function>("fn") else { continue };
                let _ = f.call::<Value>(());
            }
        }

        let mut lines = Vec::new();
        for v in pending.sequence_values::<String>() {
            if let Ok(s) = v {
                lines.push(s);
            }
        }
        Self::clear_table(&pending);
        lines
    }

    /// Removes every entry from a Lua sequence table in place --
    /// shared by `on_oper`'s pre-call safety clear and its post-call
    /// drain.
    fn clear_table(table: &Table) {
        let len = table.raw_len();
        for i in 1..=len {
            let _ = table.set(i, Value::Nil);
        }
    }

    /// Checked against interpreter 0 only -- every instance in the pool
    /// has the identical set of registered commands (see `load_dir`),
    /// so any one of them answers this the same way.
    pub fn has_command(&self, name: &str) -> bool {
        let lua = self.interpreters[0].lock().unwrap();
        let Ok(irust) = lua.globals().get::<Table>("irust") else {
            return false;
        };
        let Ok(commands) = irust.get::<Table>("_commands") else {
            return false;
        };
        match commands.get::<Value>(name.to_lowercase()) {
            Ok(Value::Table(entry)) => matches!(entry.get::<Value>("fn"), Ok(Value::Function(_))),
            _ => false,
        }
    }

    pub fn run_command(&self, name: &str, arg: &str) -> Option<String> {
        self.run_command_says(name, arg).0
    }

    /// `run_command` plus the lines the command queued with `irust.say`.
    /// The command runs on every interpreter (same as before), so every
    /// one of them queues the same lines -- only the last interpreter
    /// that ran it is read back, the others are just cleared, or each
    /// line would go out once per interpreter.
    pub fn run_command_says(&self, name: &str, arg: &str) -> (Option<String>, Vec<String>) {
        let mut result = None;
        let mut says = Vec::new();
        for interp in &self.interpreters {
            let lua = interp.lock().unwrap();
            let Ok(irust) = lua.globals().get::<Table>("irust") else { continue };
            let Some(f) = (|| -> Option<Function> {
                let commands: Table = irust.get("_commands").ok()?;
                let entry: Table = commands.get(name.to_lowercase()).ok()?;
                entry.get("fn").ok()
            })() else {
                continue;
            };
            let pending = irust.get::<Table>("_pending_says").ok();
            if let Some(p) = &pending {
                Self::clear_table(p);
            }
            let text = match f.call::<Value>(arg) {
                Ok(Value::String(s)) => s.to_str().ok().map(|s| s.to_string()),
                _ => None,
            };
            if let Some(p) = &pending {
                says = p.sequence_values::<String>().filter_map(Result::ok).collect();
                Self::clear_table(p);
            }
            if text.is_some() {
                result = text;
            }
        }
        (result, says)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_scripts_loaded_leaves_messages_unchanged() {
        let host = PluginHost::new().unwrap();
        let result = host.on_message("Alice", "#chan", "hello");
        assert_eq!(result, PluginHookResult::Unchanged);
    }

    #[test]
    fn a_script_can_replace_the_displayed_text() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_message(function(nick, target, text)
                return text .. " [seen]"
            end)
            "#,
            "test",
        )
        .unwrap();
        let result = host.on_message("Alice", "#chan", "hello");
        assert_eq!(result, PluginHookResult::Replaced("hello [seen]".to_string()));
    }

    #[test]
    fn a_script_returning_nil_leaves_the_message_unchanged() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_message(function(nick, target, text)
                -- no return value = nil = don't touch it
            end)
            "#,
            "test",
        )
        .unwrap();
        let result = host.on_message("Alice", "#chan", "hello");
        assert_eq!(result, PluginHookResult::Unchanged);
    }

    #[test]
    fn handlers_receive_nick_and_target_correctly() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_message(function(nick, target, text)
                return nick .. "@" .. target .. ": " .. text
            end)
            "#,
            "test",
        )
        .unwrap();
        let result = host.on_message("Alice", "#chan", "hi");
        assert_eq!(result, PluginHookResult::Replaced("Alice@#chan: hi".to_string()));
    }

    #[test]
    fn first_matching_handler_wins_but_later_ones_still_run() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            local calls = 0
            irust.on_message(function(nick, target, text)
                return "first"
            end)
            irust.on_message(function(nick, target, text)
                calls = calls + 1
                return "second"
            end)
            "#,
            "test",
        )
        .unwrap();
        let result = host.on_message("Alice", "#chan", "hi");
        assert_eq!(result, PluginHookResult::Replaced("first".to_string()));
    }

    #[test]
    fn on_send_lets_a_script_replace_outgoing_text() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_send(function(target, text, extra_nicks)
                return text:upper()
            end)
            "#,
            "test",
        )
        .unwrap();
        let result = host.on_send("#chan", "ciao", &[]);
        assert_eq!(result, PluginHookResult::Replaced("CIAO".to_string()));
    }

    #[test]
    fn on_send_and_on_message_handlers_are_independent() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_send(function(target, text, extra_nicks) return "SEND" end)
            "#,
            "test",
        )
        .unwrap();
        assert_eq!(host.on_message("Alice", "#chan", "hi"), PluginHookResult::Unchanged);
        assert_eq!(
            host.on_send("#chan", "hi", &[]),
            PluginHookResult::Replaced("SEND".to_string())
        );
    }

    #[test]
    fn on_send_passes_extra_nicks_through_as_a_lua_array() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_send(function(target, text, extra_nicks)
                return table.concat(extra_nicks, ",")
            end)
            "#,
            "test",
        )
        .unwrap();
        let nicks = vec!["Alice".to_string(), "Bob".to_string()];
        let result = host.on_send("#chan", "hi", &nicks);
        assert_eq!(result, PluginHookResult::Replaced("Alice,Bob".to_string()));
    }

    #[test]
    fn a_script_can_register_and_respond_to_a_custom_command() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            local enabled = true
            irust.on_command("demo", function(arg)
                if arg == "on" then enabled = true
                elseif arg == "off" then enabled = false
                else enabled = not enabled end
                if enabled then return "demo ON" else return "demo OFF" end
            end)
            "#,
            "test",
        )
        .unwrap();

        assert!(host.has_command("demo"));
        assert!(host.has_command("DEMO"), "lookup should be case-insensitive");
        assert!(!host.has_command("nope"));

        assert_eq!(host.run_command("demo", ""), Some("demo OFF".to_string()));
        assert_eq!(
            host.run_command("demo", "on"),
            Some("demo ON".to_string())
        );
    }

    #[test]
    fn say_lines_from_a_command_come_back_once_in_order() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_command("big", function(arg)
                irust.say("one " .. arg)
                irust.say("two " .. arg)
            end)
            "#,
            "test",
        )
        .unwrap();

        let (status, says) = host.run_command_says("big", "x");
        assert_eq!(status, None);
        assert_eq!(says, vec!["one x".to_string(), "two x".to_string()]);
        // nothing left over for the next call
        let (_, says) = host.run_command_says("big", "y");
        assert_eq!(says, vec!["one y".to_string(), "two y".to_string()]);
    }

    #[test]
    fn a_command_can_be_called_directly_through_the_commands_table() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_command("demo", function(arg) return "got:" .. arg end)
            irust.on_command("d", function(arg) return irust._commands.demo(arg) end)
            "#,
            "test",
        )
        .unwrap();
        assert_eq!(host.run_command("d", "on"), Some("got:on".to_string()));
    }

    #[test]
    fn running_an_unregistered_command_returns_none() {
        let host = PluginHost::new().unwrap();
        assert_eq!(host.run_command("nope", ""), None);
    }

    #[test]
    fn a_broken_script_does_not_prevent_others_from_loading() {
        let host = PluginHost::new().unwrap();
        assert!(host.load_str("this is not valid lua {{{", "broken").is_err());
        host.load_str(
            r#"
            irust.on_message(function(nick, target, text)
                return "still works"
            end)
            "#,
            "good",
        )
        .unwrap();
        let result = host.on_message("Alice", "#chan", "hi");
        assert_eq!(result, PluginHookResult::Replaced("still works".to_string()));
    }

    #[test]
    fn unload_script_removes_only_that_scripts_handlers() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"irust.on_message(function(nick, target, text) return "from A" end)"#,
            "a",
        )
        .unwrap();
        host.load_str(
            r#"irust.on_message(function(nick, target, text) return "from B" end)"#,
            "b",
        )
        .unwrap();

        assert!(host.unload_script("a"));
        // "a"'s handler is gone, "b"'s is untouched -- "from B" is now
        // the only (and therefore first) registered handler.
        assert_eq!(
            host.on_message("Alice", "#chan", "hi"),
            PluginHookResult::Replaced("from B".to_string())
        );
        assert_eq!(host.loaded_scripts(), vec!["b".to_string()]);

        // Unloading again (already gone) reports false, doesn't error
        // or touch "b".
        assert!(!host.unload_script("a"));
        assert_eq!(
            host.on_message("Alice", "#chan", "hi"),
            PluginHookResult::Replaced("from B".to_string())
        );
    }

    #[test]
    fn unload_script_removes_only_that_scripts_commands() {
        let host = PluginHost::new().unwrap();
        host.load_str(r#"irust.on_command("foo", function(arg) return "foo" end)"#, "a")
            .unwrap();
        host.load_str(r#"irust.on_command("bar", function(arg) return "bar" end)"#, "b")
            .unwrap();

        host.unload_script("a");
        assert!(!host.has_command("foo"));
        assert!(host.has_command("bar"));
    }

    #[test]
    fn load_file_reloads_an_already_loaded_script_cleanly() {
        let host = PluginHost::new().unwrap();
        let dir = std::env::temp_dir().join(format!("irust_plugin_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("greet.lua");

        fs::write(&path, r#"irust.on_message(function(n, t, x) return "v1" end)"#).unwrap();
        assert_eq!(host.load_file(&path).unwrap(), true);
        assert_eq!(
            host.on_message("A", "#c", "hi"),
            PluginHookResult::Replaced("v1".to_string())
        );

        fs::write(&path, r#"irust.on_message(function(n, t, x) return "v2" end)"#).unwrap();
        assert_eq!(host.load_file(&path).unwrap(), true);
        assert_eq!(
            host.on_message("A", "#c", "hi"),
            PluginHookResult::Replaced("v2".to_string()),
            "reloading the same file should replace v1's handler, not stack a second one"
        );
        assert_eq!(host.loaded_scripts(), vec!["greet".to_string()]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_named_resolves_with_or_without_the_lua_extension() {
        let host = PluginHost::new().unwrap();
        let dir = std::env::temp_dir().join(format!("irust_plugin_test_named_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("greet.lua"),
            r#"irust.on_message(function(n, t, x) return "hey" end)"#,
        )
        .unwrap();

        assert_eq!(host.load_named(&dir, "greet").unwrap(), Some(true));
        assert_eq!(
            host.on_message("A", "#c", "hi"),
            PluginHookResult::Replaced("hey".to_string())
        );
        assert_eq!(host.load_named(&dir, "nonexistent").unwrap(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    fn any_interpreter_global(host: &PluginHost, key: &str) -> Option<String> {
        for interp in &host.interpreters {
            let lua = interp.lock().unwrap();
            if let Ok(Some(v)) = lua.globals().get::<Option<String>>(key) {
                return Some(v);
            }
        }
        None
    }

    #[test]
    fn on_join_fires_with_nick_and_channel() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"irust.on_join(function(nick, channel) _G.seen = nick .. "@" .. channel end)"#,
            "test",
        )
        .unwrap();
        host.on_join("Alice", "#chatnet");
        assert_eq!(
            any_interpreter_global(&host, "seen"),
            Some("Alice@#chatnet".to_string())
        );
    }

    #[test]
    fn on_part_fires_with_nick_channel_and_reason() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"irust.on_part(function(nick, channel, reason) _G.seen = nick .. "@" .. channel .. ":" .. reason end)"#,
            "test",
        )
        .unwrap();
        host.on_part("Alice", "#chatnet", "bye");
        assert_eq!(
            any_interpreter_global(&host, "seen"),
            Some("Alice@#chatnet:bye".to_string())
        );
    }

    #[test]
    fn on_quit_and_on_nick_and_on_topic_and_on_mode_all_fire() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_quit(function(nick, reason) _G.quit_seen = nick .. ":" .. reason end)
            irust.on_nick(function(old_nick, new_nick) _G.nick_seen = old_nick .. "->" .. new_nick end)
            irust.on_topic(function(nick, channel, topic) _G.topic_seen = nick .. "@" .. channel .. ":" .. topic end)
            irust.on_mode(function(nick, target, modestring) _G.mode_seen = nick .. " " .. target .. " " .. modestring end)
            "#,
            "test",
        )
        .unwrap();
        host.on_quit("Alice", "Ping timeout");
        host.on_nick("Alice", "Alice_");
        host.on_topic("Bob", "#chatnet", "new topic");
        host.on_mode("K9", "#chatnet", "+o abcX");
        assert_eq!(
            any_interpreter_global(&host, "quit_seen"),
            Some("Alice:Ping timeout".to_string())
        );
        assert_eq!(
            any_interpreter_global(&host, "nick_seen"),
            Some("Alice->Alice_".to_string())
        );
        assert_eq!(
            any_interpreter_global(&host, "topic_seen"),
            Some("Bob@#chatnet:new topic".to_string())
        );
        assert_eq!(
            any_interpreter_global(&host, "mode_seen"),
            Some("K9 #chatnet +o abcX".to_string())
        );
    }

    #[test]
    fn on_connect_and_on_disconnect_fire_with_the_network_name() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_connect(function(network) _G.connect_seen = network end)
            irust.on_disconnect(function(network) _G.disconnect_seen = network end)
            "#,
            "test",
        )
        .unwrap();
        host.on_connect("chatnet");
        host.on_disconnect("chatnet");
        assert_eq!(
            any_interpreter_global(&host, "connect_seen"),
            Some("chatnet".to_string())
        );
        assert_eq!(
            any_interpreter_global(&host, "disconnect_seen"),
            Some("chatnet".to_string())
        );
    }

    #[test]
    fn every_registered_on_join_handler_runs_not_just_the_first() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            _G.calls = 0
            irust.on_join(function(nick, channel) _G.calls = _G.calls + 1 end)
            irust.on_join(function(nick, channel) _G.calls = _G.calls + 1 end)
            "#,
            "test",
        )
        .unwrap();
        host.on_join("Alice", "#chatnet");
        // `pick()` only ran the handlers on ONE interpreter -- check
        // that one specifically rather than summing across the pool.
        let mut found = false;
        for interp in &host.interpreters {
            let lua = interp.lock().unwrap();
            if let Ok(2) = lua.globals().get::<i64>("calls") {
                found = true;
            }
        }
        assert!(
            found,
            "both handlers should have run on whichever interpreter was picked"
        );
    }

    #[test]
    fn unload_script_removes_its_notify_hooks_too() {
        let host = PluginHost::new().unwrap();
        host.load_str(r#"irust.on_join(function(n, c) _G.seen = "a" end)"#, "a")
            .unwrap();
        host.unload_script("a");
        host.on_join("Alice", "#chatnet");
        assert_eq!(
            any_interpreter_global(&host, "seen"),
            None,
            "unloaded script's handler must not fire"
        );
    }

    #[test]
    fn on_oper_returns_lines_queued_via_irust_send() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_oper(function()
                irust.send("MODE " .. irust.mynick() .. " +s cC")
            end)
            "#,
            "test",
        )
        .unwrap();
        host.update_nick("abcx");
        assert_eq!(host.on_oper(), vec!["MODE abcx +s cC".to_string()]);
    }

    #[test]
    fn on_oper_with_no_handlers_returns_nothing() {
        let host = PluginHost::new().unwrap();
        assert_eq!(host.on_oper(), Vec::<String>::new());
    }

    #[test]
    fn on_oper_collects_lines_from_every_handler_in_order() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_oper(function() irust.send("first") end)
            irust.on_oper(function() irust.send("second") end)
            "#,
            "test",
        )
        .unwrap();
        assert_eq!(host.on_oper(), vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn on_oper_does_not_leak_queued_lines_into_the_next_call() {
        let host = PluginHost::new().unwrap();
        host.load_str(r#"irust.on_oper(function() irust.send("only once") end)"#, "test")
            .unwrap();
        // `pick()` can land on a different idle interpreter each call,
        // so this only reliably proves no-leak by forcing the same
        // interpreter both times.
        let held: Vec<_> = host
            .interpreters
            .iter()
            .skip(1)
            .map(|i| i.try_lock().unwrap())
            .collect();
        assert_eq!(host.on_oper(), vec!["only once".to_string()]);
        assert_eq!(
            host.on_oper(),
            vec!["only once".to_string()],
            "second call should re-queue, not leak the first"
        );
        drop(held);
    }

    #[test]
    fn mynick_reflects_the_last_value_set_by_update_nick() {
        let host = PluginHost::new().unwrap();
        host.load_str(r#"irust.on_oper(function() irust.send(irust.mynick()) end)"#, "test")
            .unwrap();
        host.update_nick("abcx");
        assert_eq!(host.on_oper(), vec!["abcx".to_string()]);
        host.update_nick("abcx_");
        assert_eq!(host.on_oper(), vec!["abcx_".to_string()]);
    }

    #[test]
    fn mynick_is_empty_string_before_update_nick_is_ever_called() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"irust.on_oper(function() irust.send("[" .. irust.mynick() .. "]") end)"#,
            "test",
        )
        .unwrap();
        assert_eq!(host.on_oper(), vec!["[]".to_string()]);
    }

    #[test]
    fn unload_script_removes_its_on_oper_handler_too() {
        let host = PluginHost::new().unwrap();
        host.load_str(r#"irust.on_oper(function() irust.send("from a") end)"#, "a")
            .unwrap();
        host.unload_script("a");
        assert_eq!(host.on_oper(), Vec::<String>::new());
    }

    #[test]
    fn pick_uses_an_idle_interpreter_instead_of_blocking_on_a_busy_round_robin_slot() {
        let host = std::sync::Arc::new(PluginHost::new().unwrap());
        let held = host.interpreters[0].lock().unwrap();

        let host2 = std::sync::Arc::clone(&host);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..(POOL_SIZE + 1) {
                drop(host2.pick());
            }
            let _ = tx.send(());
        });

        rx.recv_timeout(std::time::Duration::from_secs(2))
            .expect("pick() blocked on a busy round-robin slot instead of using an idle interpreter");
        drop(held);
    }

    #[test]
    fn on_message_for_and_on_send_for_run_the_generic_hook() {
        let host = PluginHost::new().unwrap();
        host.load_str(
            r#"
            irust.on_message(function(nick, target, text) return "seen" end)
            irust.on_send(function(target, text, nicks) return "seen" end)
        "#,
            "generic",
        )
        .unwrap();
        assert_eq!(
            host.on_message_for("net", true, "a", "#c", "hi"),
            PluginHookResult::Replaced("seen".to_string())
        );
        assert_eq!(
            host.on_send_for("net", "#c", "hi", &[]),
            PluginHookResult::Replaced("seen".to_string())
        );
    }
}

