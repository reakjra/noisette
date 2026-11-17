game save backup tool for wine prefixes and native linux games.


### Usage
```
noisette list --prefix ~/Games/prefixes/MyHugePrefix # list found saves in the prefix
noisette export --prefix ~/Games/prefixes/MyHugePrefix --out ~/saves # export all the found saves into a folder
noisette list --from ~/saves
noisette import --from ~/saves --prefix ~/Games/prefixes/MyTinyPrefix --dry-run
noisette import --from ~/saves --prefix ~/Games/prefixes/MyTinyPrefix --game 1245620 # only import in the prefix the saves from game with id 1245620
```

`--game` takes the manifest name or a steam/gog id and can be repeated

backups are one folder per game

```
~/saves/Elden Ring/
  noisette.toml
  files/winAppData/EldenRing/76561198797604835/ER0000.sl2
```

General idea how to use it, `--help` for additional commands/options.

### Build
```
git clone https://github.com/reakjra/noisette && cd noisette
cargo build --release
```

save locations come from [PCGamingWiki](https://www.pcgamingwiki.com) through ludusavi-manifest. the manifest is downloaded on first run and cached in `~/.cache/noisette`

### License
GPL3-3.0-or-later
