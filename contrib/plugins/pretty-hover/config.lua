-- Configuration module for pretty-hover plugin
-- Defines default detection patterns and styling for Doxygen tags

local M = {}

-- Default configuration
M.defaults = {
    -- Tables grouping the detected strings and using the markdown highlighters
    header = {
        detect = { "[\\@]class" },
        styler = '###',
    },
    line = {
        detect = { "[\\@]brief" },
        styler = '**',
    },
    listing = {
        detect = { "[\\@]li" },
        styler = " - ",
    },
    references = {
        detect = { "[\\@]ref", "[\\@]c", "[\\@]name" },
        styler = { "**", "`" },
    },
    group = {
        detect = {
            -- [Group name] = {detectors}
            ["Parameters"] = { "[\\@]param", "[\\@]*param*" },
            ["Types"] = { "[\\@]tparam" },
            ["See"] = { "[\\@]see" },
            ["Return Value"] = { "[\\@]retval" },
        },
        styler = "`",
    },

    -- Tables used for cleaner identification of hover segments
    code = {
        start = { "[\\@]code" },
        ending = { "[\\@]endcode" },
    },
    return_statement = {
        "[\\@]return",
        "[\\@]*return*",
    },

    -- Highlight groups used in the hover method
    hl = {
        error = {
            color = "#DC2626",
            detect = { "[\\@]error", "[\\@]bug" },
            line = false, -- Flag detecting if the whole line should be highlighted
        },
        warning = {
            color = "#FBBF24",
            detect = { "[\\@]warning", "[\\@]thread_safety", "[\\@]throw" },
            line = false,
        },
        info = {
            color = "#2563EB",
            detect = { "[\\@]remark", "[\\@]note", "[\\@]notes" },
            line = false,
        },
    },

    -- Plugin behavior options
    border = {
        -- Simple solid color border (fallback)
        color = "ui.text.focus",

        -- Gradient border configuration
        gradient = {
            -- Colors for the gradient (supports hex colors)
            colors = { "#dc8a78", "#1e66f5" },
            -- Gradient direction: "horizontal", "vertical", "diagonal", "radial"
            direction = "horizontal",
            -- Number of interpolation steps for smoother gradients
            steps = 10
        },

        -- Border style: "rounded", "all", "top", "bottom", "left", "right", or "none"
        style = "rounded",

        -- Border thickness (1-5): 1=thin, 2=thick, 3=double, 4=block, 5=full
        thickness = 1,

        -- Animation speed (0 = no animation, higher = faster)
        animation_speed = 0
    },
    wrap = true,
    max_width = nil,
    max_height = nil,
    multi_server = true,
    enabled = true,
    number_conversion = true, -- Enable number conversion feature
}

-- Merge user config with defaults
function M.setup(user_config)
    local config = {}
    
    -- Deep copy defaults
    for k, v in pairs(M.defaults) do
        if type(v) == "table" then
            config[k] = {}
            for k2, v2 in pairs(v) do
                config[k][k2] = v2
            end
        else
            config[k] = v
        end
    end
    
    -- Override with user config
    if user_config then
        for k, v in pairs(user_config) do
            if type(v) == "table" and type(config[k]) == "table" then
                for k2, v2 in pairs(v) do
                    config[k][k2] = v2
                end
            else
                config[k] = v
            end
        end
    end
    
    return config
end

-- Preset configurations for easy styling
M.presets = {
    -- Minimal clean look
    minimal = {
        border = {
            color = "ui.text",
            gradient = nil,
            style = "all",
            thickness = 1,
            animation_speed = 0
        }
    },

    -- Colorful gradient theme
    colorful = {
        border = {
            gradient = {
                colors = { "#FF6B6B", "#4ECDC4", "#45B7D1", "#96CEB4", "#FFEAA7" },
                direction = "horizontal",
                steps = 20
            },
            style = "rounded",
            thickness = 1,
            animation_speed = 0
        }
    },

    -- Ocean theme
    ocean = {
        border = {
            gradient = {
                colors = { "#1e3a8a", "#3b82f6", "#06b6d4", "#0891b2" },
                direction = "vertical",
                steps = 15
            },
            style = "rounded",
            thickness = 2
        }
    },

    -- Sunset theme
    sunset = {
        border = {
            gradient = {
                colors = { "#fbbf24", "#f59e0b", "#dc2626", "#7c2d12" },
                direction = "diagonal",
                steps = 12
            },
            style = "rounded",
            thickness = 1
        }
    },

    -- Animated rainbow
    rainbow = {
        border = {
            gradient = {
                colors = { "#ff0000", "#ff8000", "#ffff00", "#80ff00", "#00ff00", "#00ff80", "#00ffff", "#0080ff", "#0000ff", "#8000ff", "#ff00ff", "#ff0080" },
                direction = "horizontal",
                steps = 24
            },
            style = "rounded",
            thickness = 1,
            animation_speed = 2
        }
    },

    -- Retro terminal theme
    retro = {
        border = {
            gradient = {
                colors = { "#00ff41", "#39ff14" },
                direction = "vertical",
                steps = 8
            },
            style = "all",
            thickness = 3
        }
    }
}

-- Function to apply a preset
function M.setup_preset(preset_name, user_config)
    local config = M.setup(user_config)

    if M.presets[preset_name] then
        for k, v in pairs(M.presets[preset_name]) do
            if type(v) == "table" and type(config[k]) == "table" then
                for k2, v2 in pairs(v) do
                    if type(v2) == "table" and type(config[k][k2]) == "table" then
                        for k3, v3 in pairs(v2) do
                            config[k][k2][k3] = v3
                        end
                    else
                        config[k][k2] = v2
                    end
                end
            else
                config[k] = v
            end
        end
    end

    return config
end

return M
