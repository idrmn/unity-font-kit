New fonts for {game}

Install
1. Close the game.
2. Unpack this archive into the game folder: the folder with the game's .exe file and the {folder} folder.
3. Windows: double-click "Install fonts.bat" in the unpacked folder, not inside the zip.
   Linux or Steam Deck: open a terminal in the game folder and run: sh install.sh
4. Start the game and check the text.

If you unpacked the archive somewhere else, the script asks for the game folder.
Drag the game folder into its window and press Enter.

If the game is in Program Files and the script says it cannot change the files,
right-click "Install fonts.bat" and choose "Run as administrator".

Keep the ufk-patch folder after you install: it holds the original game files that Undo puts back.

Undo
Windows: double-click "Restore original files.bat".
Linux or Steam Deck: run: sh restore.sh

The patch fits only the game version it was made for.
If your game files are another version, the script changes nothing and lists those files.
After a game update, install the patch made for the new version.

Made with unity-font-kit: https://github.com/idrmn/unity-font-kit
The patch is applied with xdelta3decode from Xdelta 3.2.2 (https://github.com/jmacd/xdelta),
Apache License 2.0, see ufk-patch/LICENSE-xdelta.txt.
