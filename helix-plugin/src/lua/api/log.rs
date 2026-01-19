use crate::error::Result;
use log::{debug, error, info, trace, warn};
use mlua::prelude::*;

pub fn register_log_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let log_module = lua.create_table()?;

    // helix.log.info(message)
    let info_fn = lua.create_function(|_lua, message: String| {
        info!("[plugin] {}", message);
        Ok(())
    })?;
    log_module.set("info", info_fn)?;

    // helix.log.warn(message)
    let warn_fn = lua.create_function(|_lua, message: String| {
        warn!("[plugin] {}", message);
        Ok(())
    })?;
    log_module.set("warn", warn_fn)?;

    // helix.log.error(message)
    let error_fn = lua.create_function(|_lua, message: String| {
        error!("[plugin] {}", message);
        Ok(())
    })?;
    log_module.set("error", error_fn)?;

    // helix.log.debug(message)
    let debug_fn = lua.create_function(|_lua, message: String| {
        debug!("[plugin] {}", message);
        Ok(())
    })?;
    log_module.set("debug", debug_fn)?;

    // helix.log.trace(message)
    let trace_fn = lua.create_function(|_lua, message: String| {
        trace!("[plugin] {}", message);
        Ok(())
    })?;
    log_module.set("trace", trace_fn)?;

    helix_table.set("log", log_module)?;

    Ok(())
}
