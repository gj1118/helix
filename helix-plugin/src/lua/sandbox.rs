//! Restrictions of the plugin Lua state beyond the time limit (see the parent module).

use mlua::prelude::*;
use std::path::Path;

/// The standard libraries plugins get. `io` is never loaded (and so cannot be fetched back
/// with `require("io")`), `debug` is unsafe, and C modules are disabled.
pub(super) fn libs() -> LuaStdLib {
    LuaStdLib::COROUTINE
        | LuaStdLib::TABLE
        | LuaStdLib::OS
        | LuaStdLib::STRING
        | LuaStdLib::UTF8
        | LuaStdLib::MATH
        | LuaStdLib::PACKAGE
}

/// Elements a `table.insert`/`table.remove`/`table.move` call may shift in C. Larger
/// shifts run as Lua loops, which the time limit can interrupt: the C loops cannot be, and a
/// table can claim any length (`__len`) and `table.move` takes any range.
const MAX_C_SHIFT: i64 = 4096;

const SANDBOX: &str = r##"
local MAX_C_SHIFT = ...
local raw_setmetatable, rawget, type, error = setmetatable, rawget, type, error
local raw_insert, raw_remove, raw_move = table.insert, table.remove, table.move
local select, tointeger, maxinteger = select, math.tointeger, math.maxinteger

-- No processes, file system changes or process-wide settings.
os.execute = nil
os.exit = nil
os.remove = nil
os.rename = nil
os.tmpname = nil
os.setlocale = nil
loadfile = nil
dofile = nil

-- `require` only finds modules in the plugin directories (added as plugins are loaded),
-- never in the working directory, which may be an untrusted project.
package.path = ""
package.cpath = ""

-- Lua runs finalizers with the instruction hook switched off, so a looping `__gc` could
-- never be interrupted, not even when the state is closed on reload or exit.
setmetatable = function(t, mt)
    if type(mt) == "table" and rawget(mt, "__gc") ~= nil then
        error("__gc metamethods are not available to plugins", 2)
    end
    return raw_setmetatable(t, mt)
end

table.insert = function(t, ...)
    if select("#", ...) == 2 and type(t) == "table" then
        local pos, value = ...
        local e, p = tointeger(#t), tointeger(pos)
        if e and p and e - p >= MAX_C_SHIFT then
            if p < 1 then
                error("bad argument #2 to 'insert' (position out of bounds)", 2)
            end
            for i = e + 1, p + 1, -1 do
                t[i] = t[i - 1]
            end
            t[p] = value
            return
        end
    end
    return raw_insert(t, ...)
end

table.remove = function(t, ...)
    if select("#", ...) >= 1 and type(t) == "table" then
        local size, p = tointeger(#t), tointeger((...))
        if size and p and size - p >= MAX_C_SHIFT then
            if p < 1 then
                error("bad argument #2 to 'remove' (position out of bounds)", 2)
            end
            local value = t[p]
            for i = p, size - 1 do
                t[i] = t[i + 1]
            end
            t[size] = nil
            return value
        end
    end
    return raw_remove(t, ...)
end

table.move = function(a1, f, e, t, a2)
    local fi, ei, ti = tointeger(f), tointeger(e), tointeger(t)
    -- (`ei - fi` wraps around to a negative number when the range is too large: the
    -- original reports that)
    if fi and ei and ti and ei - fi >= MAX_C_SHIFT and ei - fi < maxinteger then
        raw_move(a1, 1, 0, ti, a2) -- argument checks
        local n = ei - fi + 1
        if ti > maxinteger - n + 1 then
            error("bad argument #4 to 'move' (destination wrap around)", 2)
        end
        local dst = a2
        if dst == nil then
            dst = a1
        end
        if ti > ei or ti <= fi or (a2 ~= nil and a1 ~= a2) then
            for i = 0, n - 1 do
                dst[ti + i] = a1[fi + i]
            end
        else
            for i = n - 1, 0, -1 do
                dst[ti + i] = a1[fi + i]
            end
        end
        return dst
    end
    return raw_move(a1, f, e, t, a2)
end
"##;

/// Apply the restrictions to a fresh plugin state.
pub(super) fn restrict(lua: &Lua) -> LuaResult<()> {
    lua.load(SANDBOX)
        .set_name("=sandbox")
        .call::<()>(MAX_C_SHIFT)?;

    // `print` and `warn` would write to the terminal Helix is drawing on.
    let print = lua.create_function(|lua, args: LuaMultiValue| {
        let mut line = String::new();
        for (i, value) in args.into_iter().enumerate() {
            if i > 0 {
                line.push('\t');
            }
            match value {
                LuaValue::String(_) | LuaValue::Integer(_) | LuaValue::Number(_) => {
                    if let Some(s) = lua.coerce_string(value)? {
                        line.push_str(&s.to_string_lossy());
                    }
                }
                value => line.push_str(&value.to_string()?),
            }
        }
        log::info!("[plugin {}] {}", super::current_plugin_name(), line);
        Ok(())
    })?;
    let warn = lua.create_function(|lua, args: LuaMultiValue| {
        let mut message = Vec::new();
        for value in args.iter() {
            let s = lua.coerce_string(value.clone())?.ok_or_else(|| {
                LuaError::RuntimeError(format!(
                    "bad argument to 'warn' (string expected, got {})",
                    value.type_name()
                ))
            })?;
            message.extend_from_slice(&s.as_bytes());
        }
        // A single argument starting with '@' is a control message ("@on", "@off").
        if !(args.len() == 1 && message.first() == Some(&b'@')) {
            log::warn!(
                "[plugin {}] {}",
                super::current_plugin_name(),
                String::from_utf8_lossy(&message)
            );
        }
        Ok(())
    })?;
    lua.globals().set("print", print)?;
    lua.globals().set("warn", warn)?;

    lua.set_memory_limit(super::PLUGIN_MEMORY_LIMIT)?;
    Ok(())
}

/// Let `require` find modules in `dir` (`require("my-plugin.util")` loads
/// `<dir>/my-plugin/util.lua`).
pub(super) fn add_module_dir(lua: &Lua, dir: &Path) -> LuaResult<()> {
    let package: LuaTable = lua.globals().get("package")?;
    let path: String = package.get("path")?;
    let dir = dir.to_string_lossy();
    let entries = [format!("{dir}/?.lua"), format!("{dir}/?/init.lua")];
    if path.split(';').any(|entry| entry == entries[0]) {
        return Ok(());
    }
    let mut new_path = entries.join(";");
    if !path.is_empty() {
        new_path.push(';');
        new_path.push_str(&path);
    }
    package.set("path", new_path)
}
