#[cfg(feature = "integration")]
mod test {
    mod helpers;

    use helix_core::{syntax::config::AutoPairConfig, Selection};
    use helix_term::config::Config;

    use indoc::indoc;

    use self::helpers::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn hello_world() -> anyhow::Result<()> {
        test(("#[\n|]#", "ihello world<esc>", "hello world#[|\n]#")).await?;
        Ok(())
    }

    mod auto_pairs;
    mod blame;
    mod borders;
    mod breadcrumbs;
    mod cmdline_popup;
    mod code_lenses;
    mod command_line;
    mod command_palette;
    mod commands;
    mod completion;
    mod component_keys;
    mod custom_commands;
    mod dap;
    mod diff;
    mod editorconfig;
    mod explorer;
    mod fold_ui;
    mod fuzz;
    mod history;
    mod hlslens;
    mod image_preview;
    mod inline_completion;
    mod last_position;
    mod location_list;
    mod long_lines;
    mod lsp_data;
    mod match_highlight;
    mod mouse_yank;
    mod movement;
    mod multi_cursor;
    mod navigation;
    mod persistence;
    mod pile;
    mod plaintasks;
    mod plugin_panels;
    mod plugins;
    mod popups;
    mod prompt_completion;
    mod quickfix;
    mod quicklist;
    mod recent_files;
    #[cfg(unix)]
    mod remote;
    mod responsiveness;
    mod robustness;
    mod rounded_cursor;
    mod scrollbar;
    mod smooth_scroll;
    mod snippets;
    mod splits;
    mod tabs;
    mod tiny_terminal;
    mod todos;
    mod undo_tree;
    mod url_buffer;
    mod welcome;
    mod window_title;
    mod yank_highlight;
}
