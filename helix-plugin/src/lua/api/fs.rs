//! `helix.fs`: what plugins may learn of the file system (they cannot read or write files
//! themselves: the `io` library is not available).
use crate::error::Result;
use crate::permissions::{require, Permission};
use mlua::prelude::*;
use std::path::PathBuf;

/// `path` with `~` and environment variables expanded, relative to the working directory.
fn resolve(path: &str) -> PathBuf {
    let path = helix_stdx::path::expand(path).into_owned();
    if path.is_relative() {
        helix_stdx::env::current_working_dir().join(path)
    } else {
        path
    }
}

pub fn register_fs_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let fs = lua.create_table()?;

    // helix.fs.exists(path) - Whether `path` (a file or a directory) exists.
    let exists = lua.create_function(|lua, path: String| {
        require(lua, Permission::Read, "helix.fs.exists")?;
        Ok(resolve(&path).exists())
    })?;
    fs.set("exists", exists)?;

    // helix.fs.is_dir(path) - Whether `path` is a directory.
    let is_dir = lua.create_function(|lua, path: String| {
        require(lua, Permission::Read, "helix.fs.is_dir")?;
        Ok(resolve(&path).is_dir())
    })?;
    fs.set("is_dir", is_dir)?;

    helix.set("fs", fs)?;
    Ok(())
}
