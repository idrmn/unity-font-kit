# unity-font-kit (`ufk`)

`ufk` replaces fonts in Unity games, so a translation can use letters the game's fonts do not have.

![A game's fonts, the letters each one lacks, and the font that replaces it](docs/images/screen-game.png)

## What it does

- Finds the fonts in a game: TextMeshPro fonts and plain Unity fonts, in the game files and in asset bundles.
- Shows which of your characters each font lacks.
- Puts the new font into the game and keeps the original files, so Restore brings them back.
- Exports a patch: a zip that puts the same fonts into the same game version without `ufk`.
- Saves the game's own font files and atlases for work in a font editor.

![Choosing a font from the game or from a file](docs/images/screen-picker.png)

## Which font is which

A font's name rarely tells where the game shows it, and some games swap fonts at run time. Press **Find in game**: every font in the table then draws its number over each letter, so the text stays readable and a menu in font 3 has a small 3 over every letter. Start the game, note the numbers, and press **Restore**. It works while the game has its original fonts, so look for the fonts before you apply new ones. The numbers stay with their fonts when you sort the table or scan again.

## Share with players

After **Apply**, the changed game files are in the `ufk` data folder, laid out like the game's `_Data` folder:

```
%LOCALAPPDATA%\ufk\projects\<game>\<game>.ufk\patched           (Windows)
~/.local/share/ufk/projects/<game>/<game>.ufk/patched           (Linux)
```

`<game>` is the game's name with a short code, for example `nice-day-for-fishing-d82d825d`. Give the files inside `patched` to players to copy over theirs. They fit only the game version you made them for.

**Export patch...** makes a smaller zip instead: it holds only the differences and scripts that install and undo them, and it checks the game version first.

## How the new fonts look

`ufk` draws TextMeshPro atlases itself with FreeType, without Unity. Each image below shows a line from an atlas the Unity editor made, the same line from the atlas `ufk` made, and the difference between them, made 20 times stronger.

![SDFAA: TextMeshPro and ufk draw the same letters](docs/images/compare-sdfaa.png)

![SDFAA_HINTED: TextMeshPro and ufk draw the same letters](docs/images/compare-sdfaa-hinted.png)

![SDF16: TextMeshPro and ufk draw the same letters](docs/images/compare-sdf16.png)

![SMOOTH_HINTED: TextMeshPro and ufk draw the same letters](docs/images/compare-smooth-hinted.png)

## Download

Take `ufk-windows-x64.zip` or `ufk-linux-x64.tar.gz` from [Releases](https://github.com/idrmn/unity-font-kit/releases), unpack it and run `ufk`.

## Build

Install [Rust](https://rustup.rs), then run in the repository folder:

```sh
cargo build --release
```

The program is `target/release/ufk` (`ufk.exe` on Windows). On Windows use the MSVC toolchain. `ufk` keeps its data in `%LOCALAPPDATA%\ufk` on Windows and `~/.local/share/ufk` on Linux.

Some steps need an internet connection the first time:

- IL2CPP games: `ufk` downloads [Cpp2IL](https://github.com/SamboyCoding/Cpp2IL) to read the game's code.
- Export patch: `ufk` downloads [xdelta](https://github.com/jmacd/xdelta) to make the patches.

## License

MIT, see [LICENSE](LICENSE). Portions of this software are copyright © The FreeType Project (www.freetype.org). All rights reserved.
