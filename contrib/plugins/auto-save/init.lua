-- Example Helix Plugin: Auto-Save
-- This plugin automatically saves buffers after a period of inactivity

local M = {}

-- Plugin configuration
local config = {
    enabled = true,
    auto_format = true,
}

-- Setup function called when plugin is loaded
function M.setup(user_config)
    -- Merge user config with defaults
    if user_config then
        for k, v in pairs(user_config) do
            config[k] = v
        end
    end

    -- Register event handlers
    if config.enabled then
        helix.on("buffer_open", M.on_buffer_open)
        helix.on("buffer_post_save", M.on_buffer_save)
    end

    print("[auto-save] Plugin loaded")
end

-- Called when a buffer is opened
function M.on_buffer_open(event)
    print("[auto-save] Buffer opened")
end

-- Called after a buffer is saved
function M.on_buffer_save(event)
    print("[auto-save] Buffer saved successfully")
end

-- Initialize the plugin
M.setup()

return M
