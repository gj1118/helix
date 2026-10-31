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
    mod code_lenses;
    mod command_line;
    mod commands;
    mod component_keys;
    mod custom_commands;
    mod dap;
    mod history;
    mod image_preview;
    mod inline_completion;
    mod movement;
    mod persistence;
    mod quicklist;
    mod snippets;
    mod splits;
}
