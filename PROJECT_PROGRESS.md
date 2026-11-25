# 開発の引き継ぎ

## 目的

- Rust で Windows x64 向けの HDR ライティングツール `HDR STUDIO` を開発する。HDR Light Studio を参考に、固定カメラでモデルを見ながら HDR 環境を編集できるようにする。
- Maya からモデルとカメラを取り込み、編集した HDR 環境を Maya の V-Ray Dome Light に反映する。

## 現在の状態

### プロジェクト

- 右側のタブは左から **File → Lighting → Environment**。Lighting にレイヤー編集、Environment にプレビュー設定と **Fixed camera** を置く。更新版は `target/release/HDR_STUDIO.exe`。
- Environment の区切りを見直したビルドは、作業開始時に通常版が起動中だったため `target/file-tab/release/HDR_STUDIO.exe` に出力した。
- 右側の **File** タブに **Render size**、**Export HDR…**、Maya V-Ray のリンク・同期停止・手動同期／再開・仕上げ HDR、**Remove All Models**、**Remove Selected Camera** を配置し、操作行の幅と高さを統一した。
- **Remove All Models** はモデルと内包用 ABC を現在のプロジェクトから外し、現在のカメラと HDR レイヤーを保持する。**Remove Selected Camera** は選択中のカメラだけを外し、HDR レイヤーを保持する。選択中のカメラがなくなった場合は残っているカメラに切り替える。新規 Alembic の取り込み時には取り込み元のモデルとカメラを使う。
- レイヤーと Lighting の配置更新版は `target/release/HDR_STUDIO.exe` にビルドした。同じ場所の `hdr_abc.dll` を使う。
- ナッジ更新版は通常版が起動中だったため `target/file-tab/release/HDR_STUDIO.exe` にビルドした。同じ場所に `hdr_abc.dll` を配置した。

- **File → New Project / Save Project… / Load Project…** を追加。`.lmo` にライトの編集データ、HDR／EXR 画像の画素、Alembic 本体、固定カメラの行列・選択、適用済みサンプル時刻、照明とプレビュー設定を保存する。初回保存は `NewProject.lmo`、再保存は現在のファイル名が入り、名前は変更できる。
- **Open Alembic…** を **Import Alembic…** に改名。**Light placement** は Reflection が初期値で、Reflection と Diffuse を切り替えられる。

### Maya・V-Ray 連携

- 右側の **File → Link Maya V-Ray** で `%LOCALAPPDATA%\HDR STUDIO\vray-sync\hdr_studio_vray.hdr` に自動保存する。Maya の **Start Sync** で同じ Windows ユーザーの固定パスを監視する。
- **Render HDR for Maya…** は選択した解像度と保存先で仕上げ用 HDR を保存し、完了後に V-Ray Dome Light がそのファイルを直接参照するよう通知する。**Resume Live Sync** で選択中のライブ同期サイズに戻す。
- 同期専用 HDR は **File → Live preview size** で 350 × 175、512 × 256、700 × 350 px（起動時の既定）から選べる。ドラッグ中も、ライトの位置・明るさ・サイズなどの編集や追加・削除をバックグラウンドで書き出す。通常と仕上げ用の HDR 書き出しは **Render size** を使用する。
- HDR ファイルが更新されても Maya で手動 Reload が必要になることがあったため、Maya の File ノードと同じ **Reload** 処理を呼ぶ。V-Ray IPR の再描画要求は連続編集中も約250 ms間隔で送り、最後の更新にも送る。Rust 側は保存回数を表示し、Windows で同期用ファイルを一時的に置き換えられない場合は次の書き出しに最新の編集を持ち越して再試行する。
- Maya 側の出力を A〜G の7枚に増やして試した。遅延 Reload でおおむね反映するとのユーザー報告を受け、出力を A・B の2枚に戻した。パスの再利用が同期抜けの原因かどうかは未確認。7枚方式で保存したシーンを再接続すると、既存 File ノードを使って A・B に切り替える。
- Maya の File ノードでパスが切り替わっても、後から手動 Reload すると V-Ray IPR が復帰することが多いとの報告を受け、V-Ray Link ウィンドウに **Reload delay** スライダーを追加した。250〜2000 ms、初期値500 ms。最後の HDR 更新から設定時間が経過したら、同じ File ノードで Reload を1回実行する。
- **Start Sync** でシーン全体の V-Ray Dome Light を確認する。0灯なら1灯作成、1灯なら **Use Existing / Cancel** で確認、2灯以上なら警告して開始を取り消す。**Use Existing** のときにその `Dome Tex` 接続を同期用に切り替える。
- Maya ツールを `scripts/maya_hdr_studio.py` に統合した。`HDRStudio` シェルフの **HDR STUDIO** ボタンから PySide の **V-Ray Sync** と **Alembic Export** を使用する。同期開始後はダイアログを閉じても監視が続く。登録手順は README.md の「Maya のシェルフに登録する」を参照。アイコンは `scripts/hdr_studio_maya.png`（32 × 32 px）。
- リリース版 `target/release/HDR_STUDIO.exe` はビルド済み。`hdr_abc.dll` は exe と同じディレクトリに配置する。
- Maya からのモデル・カメラ転送は Alembic ファイル経由。リアルタイム転送は未実装。

### Alembic と 3D プレビュー

- **2026-11-18** Windows x64 用の Alembic プレビューを実装。公式 Alembic 1.8.9 と Imath 3.1.12 を静的リンクした C++ DLL `hdr_abc.dll` を Rust から C ABI で呼び出す。Ogawa ABC のポリゴンメッシュ、階層、Transform、可視性、法線、カメラを読み込み、egui / OpenGL で表示する。
- `scripts/build-native.ps1` を追加。Maya の `maya_hdr_studio.export_selected()` は選択オブジェクトやグループの子孫と指定カメラを出力し、元の選択を復元する。アプリ内のメッシュ階層一覧は削除済み。Maya が法線を省略したスムースメッシュでは頂点法線を生成し、表示と Reflection／Diffuse によるライト配置に使う。
- SH による拡散環境光と HDR テクスチャによる反射を表示する。`Ray-traced reflections` は滑らかな面で反射を1段追跡する。`HDR soft shadows` と `Show floor shadows` はモデルとプレビュー床にボケ影を表示する。
- **2026-11-19** 専用描画スレッドを導入し、操作中は直前の画像を表示する。影の方向サンプルを画素ごとに変え、半影を粒状にした。解像度倍率、影サンプル数、操作中の最小画質、アイドル時の上限を設定できる。Lighting 設定はアプリの保存領域に保存する。
- `3D Preview` をオフにすると 3D 描画を停止し、キャンバスが 2:1 のまま中央領域に広がる。`Fixed camera` は Environment の最上部にある。
- 約468万三角形のシーンで影・反射の3項目が自動オフになる問題を修正。GPU データを複数ページに分け、単一テクスチャ寸法による上限を外した。当時の修正版は `target/ray-mesh-fix/release/HDR_STUDIO.exe`。
- 元のマテリアル／テクスチャ、被写界深度、モーションブラー、アニメーション再生、HDF5 ABC、SubD 細分化は未実装。

### HDR 編集

- **Lighting** は **Light Layers** の見出し、幅いっぱいの **+ Add Light**、レイヤー一覧、プロパティの順に表示する。Gradient Background は **+ Add Light → Layer properties → Shape** から選ぶ。全天球 HDR の上端から下端へ変化する背景で、色の制御点、明るさと透明度のカーブ、Intensity を編集できる。通常のレイヤー合成、プレビュー、HDR 書き出しと Maya 同期に含める。
- **Environment** は上から **Fixed camera**、**Exposure**、**Preview Material**、**Lighting**、**Preview Quality** の見出しと区切り線で整理する。全体の Rotation は削除した。

- 2:1 の HDR／EXR は `Shape → Image` レイヤーで読み込み、画像ごとに回転、露出、`Alpha`、`Linear` を設定できる。通常・仕上げ用の HDR と Maya ライブ同期はリニア HDR の合成結果を書き出す。
- 横幅が 700 px を超える画像は、取り込みと `.lmo` 読み込み時に 700 × 350 px の編集用画像を面積平均で作る。HDR キャンバス、3D 照明、Maya ライブ同期で共有する。小さい画像は元サイズを使用する。仕上げ用 HDR、通常書き出しと `.lmo` 保存は元の RGB32F 画素を使用する。
- 新規ライトは `Round`。`Rectangle`、`Gradient` なども選択でき、色・強度・サイズ、`Hotspot` の2Dパッド、任意角度の `Hard cut` を編集できる。`3D` マッピングは極付近で色・輪郭・芯・切断線を投影し、`Flat` にも切り替えられる。
- `Falloff curve` に5種のプリセットと制御点の追加・移動・削除がある。`Linear` プリセットは直線補間を使う。`Rectangle` の `Box Gradient` は横・縦の減衰を掛け合わせ、各カーブに `Linear`／`Smooth` を指定できる。実装時は横・縦ランプとコサイン補間の資料を参照した。
- `HDR Canvas` とグレースケールの `Falloff Preview` の両方でライトをドラッグできる。無効レイヤーは画像・輪郭が消え、選択・ドラッグの対象外になる。既定の左端は Z+。`Lat-Long Guide` は X を赤、Z を青、Y を上下端の緑、赤道を中間色で示す。
- レイヤー行に形状・色・合成モード・表示状態、鍵トグル、並べ替えハンドルを表示する。右クリックで `Rename`、`Solo`／`Exit Solo`、`Delete` を選べる。Delete キー、矢印キーでの1°移動、Ctrl+Z／Ctrl+Y に対応する。
- 選択レイヤーの `Width` は Shift＋左右、`Height` は Shift＋上下で 1° ずつ、`Intensity` は Ctrl＋上下で 1 ずつ増減する。選択中の形状で編集できる項目に適用する。
- `Multiply` に `Mask`、`Blend` に `Over` と `Low Pass` を追加。`Add`、`Amplify`、`Low Pass` にレイヤーごとの `Invert` がある。`CompositeLight` は `Edit composite` で子レイヤーを編集し、`Back` で親に戻る。親はキャンバスまたは経度・緯度で移動でき、子ライトをつかんだ点はドラッグに追従する。
- 3D プレビューの `Light placement` は Reflection／Diffuse のクリックとドラッグで選択ライトを移動する。操作開始時に Shift を押している場合やライトが未選択の場合は1個追加する。
- `Default lighting` はレイヤーがゼロの起動時や最後のレイヤー削除時にオン、新規追加時にオフになる。`HDR lighting` と `HDR background` は独立して切り替えられる。

## 決定事項

- `.lmo` はバージョン付きの独自形式。JSON にライトとカメラの設定を記録し、HDR 画素を RGB32F、モデルを元の Ogawa Alembic として内包する。Import 時に Alembic のコピーを持つため、元ファイルがなくなっても保存・復元できる。保存・読み込みは専用スレッドで行い、保存完了後に一時ファイルを保存先へ置き換える。
- 固定カメラの画角を優先する。回転・パン・ズーム操作は設けず、ウィンドウのリサイズ時も画枠のアスペクト比を維持する。
- Maya の最終レンダー画枠は補助 `.camera.json` で投影・View 行列、解像度、ピクセルアスペクトを渡す。Maya の `getRenderingFrustum` と `postProjectionMatrix` を使用する。ABC と JSON の対応はファイルサイズ・更新時刻・サンプル時刻・カメラパスで検証し、不一致はエラーとする。
- ABC 単体はフィルムゲートのプレビューとして扱い、最終レンダー画角が未確認であることを UI に表示する。カメラがない場合は視点を自動生成しない。
- モデル転送には公式 C++ 実装の Alembic / Ogawa を使う。HDR 同期はローカルの専用ファイルを介し、HDR STUDIO から Maya への一方向とする。
- モデルを外したプロジェクトではカメラ行列を ABC と独立して `.lmo` に保存する。個別に外したカメラはパスを記録し、同じモデルの Reload と `.lmo` の読み込み時に除外する。既存の `.lmo` で新規項目がない場合は既定値を使う。
- V-Ray の送り手は Windows ユーザーごとに1つ。仕上げ用 HDR は V-Ray と方位を合わせ、アトミックに保存した後に `target.json` を置き換える。Maya はこの通知で参照パスを切り替え、設定した遅延後に Reload を実行する。仕上げ用パスは **Resume Live Sync** まで保持する。
- Maya のシェルフ登録は旧ボタンと同じ識別タグを使用し、再登録で既存ボタンの実行先を統合版に変更する。Alembic Export は既定で Maya の選択モデルを対象にし、シーン全体とカメラの指定にも対応する。
- ドームの振り分けはシーン内の灯数を基準にする。確認の既定は **Cancel** とし、承認直後に対象が同じか再確認する。ドーム新設後の接続に失敗した場合は、その操作で作ったドームを取り除く。
- 各編集フレームで HDR の合成に使う状態を確認し、書き出しはワーカースレッドで直列に実行する。自動書き出しの開始間隔を最低120 msとし、処理中の編集は最新の状態を次回に反映する。同期用 HDR の水平画素を半周ずらし、一時ファイルから置き換える。一時的な置き換え失敗が続いたら250 ms空けて再試行する。Maya 側は100 ms間隔で変更を監視し、次に使う HDR の保存後にドームのパスを切り替える。Maya 標準の `AEfileTextureReloadCmd` で該当する File ノードを更新し、再描画要求を約250 ms間隔にまとめて `vrayRefreshIprImage` を呼ぶ。Maya はドームとプロセスごとに A・B の2枚を交互に再利用し、同期停止後もドームは最後に適用した HDR を参照する。
- Image の編集用画像は元画像と一緒に共有し、`Linear` をオフにしたときだけ、sRGB をリニア化してから縮小した編集用画像を初回使用時に作る。元画像は仕上げ用にメモリに保持する。
- Canvas Exposure は HDR Canvas のトーンマッピング直前、Render Preview Exposure は OpenGL プレビューの表示変換直前に適用する。表示用 EV は Lighting 設定と `.lmo` に保存する。旧 `.lmo` の全体の Exposure・Rotation は適用せず、表示用 EV が省略されている場合は 0 にする。
- サイズと Intensity のナッジは各スライダーの範囲を使い、同じ種類のキーリピートは矢印を離すまで1回の Undo にまとめる。
- Maya GUI での Reload は連続編集後の最新の1枚にまとめる。ファイルパスとドームの接続を再確認した上で、設定した遅延後に Reload を1回行い、直後に IPR 更新を要求する。スライダーは同期中も操作でき、待機中に調整した場合は新しい値で待機を始める。

## 次の作業

- Maya の GUI で統合シェルフボタンの再登録、再起動後の復元、既存ドームの確認ダイアログを確認する。実行中の V-Ray IPR で、自動同期、仕上げ用 HDR の適用、ライブ同期への復帰を制作シーンで確認する。
- 制作シーンでライブ HDR の各解像度における同期時間、ドラッグ中の 3D プレビュー更新速度、V-Ray IPR の映り込みを比較する。V-Ray GPU と Z-up の方位を検証し、制作シーンと使用レンダラーで固定カメラの画角を比較する。
- 制作 HDR 素材で CompositeLight 内のソロと編集履歴を確認する。3D 投影、Falloff、カットを編集した HDR を UI から書き出し、外部レンダラーで確認する。
- Gradient Background を含む仕上げ HDR を File タブから Maya の制作シーンに適用し、V-Ray IPR で反映を確認する。
- 制作サイズの Alembic／HDR を含む `.lmo` の保存・復元時間とファイルサイズを確認する。
- Maya からのモデル・カメラのリアルタイム転送、OCIO／ACES 対応、反射・拡散環境光の品質向上を検討する。

## 確認方法・結果

### 確認済み

- **Environment の見出し**。リリース版の実画面で Fixed camera が先頭にあり、Exposure、Preview Material、Lighting、Preview Quality が区切って表示されることを確認した。全58テストが成功した。

- **タブの統合**。リリース版の実画面で File、Lighting、Environment の順序と各操作欄の表示、検証用 Alembic のカメラと読み込み情報が Environment に表示されることを確認した。全58テストが成功した。

- **サイズと Intensity のナッジ**。キーイベントからの増減、位置を保持すること、長押しの Undo／Redo、スライダー範囲、形状別の適用、ロック中の無効化を自動テストで確認した。全58テストと Clippy が成功した。

- **表示用 Exposure の分離**。リリース版で Canvas Exposure が HDR Canvas だけ、Render Preview Exposure がモデルの 3D 表示だけを変えることを確認した。自動テストで表示用 EV の変更による HDR の再合成が発生しないこと、旧プロジェクトの全体露出・回転が HDR 書き出しとライブ合成に適用されないこと、表示用 EV の保存・復元を確認した。全57テスト、Clippy、リリースビルドが成功した。

- **レイヤーと Lighting の配置**。リリース版の実画面で Light Layers 見出しと Add Light の表示、追加したライトを Shape から Gradient Background に変更する操作、Lighting の表示順、Exposure の変更による HDR Canvas の更新を確認した。全55テストと Clippy が成功した。

- **Image レイヤーの編集用画像とライブ解像度**。全55テストと Clippy が成功した。面積平均で小さいハイライトが残ること、sRGB をリニアに変換してから平均すること、仕上げでは元画素を使うこと、同期サイズ変更後に最新の編集が書き出されることをテストした。合成のみを8Kの合成画像1枚で測定し、700 × 350 px への合成は元解像度参照で約43 ms、編集用画像参照で約40 ms、350 × 175 px への合成は約11 ms。編集用画像の初回生成は約189 ms。HDR 保存と Maya の再読み込みは計測外。

- **Gradient Background と File タブ**。実画面で背景の追加、HDR キャンバスの縦グラデーション、File タブの操作行、モデル・カメラを外した後の HDR 保持と別の ABC での照明の流用を確認した。選択カメラを外す操作ではモデルを保持し、Reload 後もカメラは除外された。自動テストで色・明るさ・透明度の合成、モデルなしの `.lmo` での背景とカメラの保存・復元、除外カメラパスの保存・復元を確認した。全54テスト、Clippy、整形チェック、別出力先でのリリースビルドが成功した。

- **自動同期パスと仕上げ用 HDR**。Rust のテストで、固定先への 700 × 350 px の保存、同一ユーザーの二重起動防止、指定解像度の HDR 保存後の通知、保存失敗時の参照維持、ライブ同期への復帰を確認。Maya 2023／2026 のバッチテストで、パス指定なしでの開始、日本語を含む仕上げ用 HDR へのパス切り替え、再接続時の File ノードの再利用、ライブ HDR への復帰を確認した。実行中の V-Ray IPR での見え方は未確認。

- **V-Ray Dome Light の灯数確認**。Maya 2023／2026 のバッチテストで、0灯時の作成と失敗時の片付け、選択対象に依存しない既存ドームの再利用、確認の承認とキャンセル、2灯時の開始拒否を確認した。確認ダイアログと警告の実画面は未確認。

- **Maya ツールの単一ファイル化**。旧3本を `scripts/maya_hdr_studio.py` に置き換えた。Maya 2023 の PySide2 と Maya 2026 の PySide6 のオフスクリーンテストで、2タブ、カメラと書き出し範囲の選択、32 × 32 px PNG の読み込み、旧シェルフボタンを差し替える登録コマンドを確認した。Maya 2026 の Alembic／選択範囲テスト、Maya 2023／2026 の V-Ray 同期テスト、Rust 全52テストが成功。統合版ボタンの Maya GUI での実登録・再起動後の復元と、制作シーンでの実行中 IPR は未確認。

- **`.lmo` 保存と読み込み**。実画面で Reflection が初期値であること、モデルへのライト配置、Import Alembic、初回と再保存の入力済みファイル名、任意名での保存、New Project、Load Project でモデル・画角・ライトが戻ることを確認した。自動テストで元 Alembic を削除した後に、内包した ABC のバイト列、カメラ行列、CompositeLight 内の HDR 画像と合成結果が一致すること、空のプロジェクトと壊れたファイルの処理を確認した。全52テスト、Clippy、整形、リリースビルドが成功。

- **A・B の2枚に戻した後の同期**。Maya 2023／V-Ray 5 と Maya 2026／V-Ray 7 のバッチテストで、A・B を繰り返し切り替えて最新の内容が入ること、旧 G のパスを参照する File ノードを再接続して A に移行できること、遅延後の Reload が1回であることを確認した。制作シーンの実行中 IPR は未確認。

- **Reload delay スライダーと単発 Reload**。Maya 2023 の PySide2 と Maya 2026 の PySide6 で、オフスクリーンのスライダーの範囲、初期値、数値表示を確認した。両バージョンの V-Ray をロードしたバッチタイマーテストで、待機中に2000 msから250 msへ調整できることと、最新の HDR に対する Reload が1回だけ実行され、直後に IPR 更新要求が送られることを確認した。制作シーンの実行中 IPR は未確認。


- **Maya の HDR 7枚切り替え**。Maya 2023／V-Ray 5 と Maya 2026／V-Ray 7 のバッチテストで、A〜G の順に切り替え、A に戻った際も最新の内容であることを確認した。同じドームへの再接続では既存の File ノードを再利用した。Maya 2023／V-Ray 5 の CPU バッチレンダーでは、最初の A と一周後の A を使ったレンダーで背景中央が赤から緑に変わった。制作シーンの実行中 IPR で同期抜けが解消するかは未確認。

- **編集中の HDR 保存と IPR 更新要求**。修正前のリリース画面でライトを移動し、同期用 HDR のファイルサイズと更新時刻が変わることを確認した。修正後の Rust テストでは、ポインターを押したまま最新の変更を保存できることと、Windows の置き換えを阻む読み取りハンドルを閉じた後に追加編集なしで保存できることを確認した。全50テスト、Clippy、整形チェック、リリースビルドが成功。Maya 2023／V-Ray 5 と Maya 2026／V-Ray 7 のタイマーテストでは、連続更新の途中と最後の HDR に対して IPR 更新要求が出ることを確認した。実行中の IPR の描画と制作シーンでの反映時間は未確認。

- **Maya の手動 Reload 相当の処理**。Maya 2023／V-Ray 5 と Maya 2026／V-Ray 7 のバッチテストで、パス変更後に Maya 標準の Reload が実行され、タイマーから最新の HDR が選ばれた状態で V-Ray の IPR 更新手続きが呼ばれることを確認した。Maya 2023／V-Ray 5 の CPU バッチレンダーで、標準 Reload を通じた差し替え後に背景が赤から緑に変わった。制作シーンの GUI で起きた症状の再現と、実行中の IPR の再描画は未確認。

- **700 × 350 px の非同期同期**。Rust のテストで、書き出し中にライトを追加・編集しても、次の HDR に最新状態が反映されることと実ファイルの寸法を確認。全49テスト、Clippy、整形チェック、リリースビルドが成功。Maya 2023／V-Ray 5 と Maya 2026／V-Ray 7 のバッチテストで、2つのパスの切り替えと各ファイルの内容を確認。Maya 2023／V-Ray 5 の CPU バッチレンダーでも、切り替え後に背景中央の画素が赤から緑に変わった。

- **Maya シェルフと PySide UI**。Maya 2023.3 の GUI で `HDRStudio` シェルフ登録、設定ファイルへの保存、**HDR** ボタンからの PySide2 ウィンドウ表示を確認。Maya 2026 の `mayapy` では PySide6 のボタン状態と監視開始・停止の呼び出しをオフスクリーンで検証し、既存の V-Ray 同期テストも成功した。
- **2026-11-24 の V-Ray 同期**。`tests/maya_vray_link.py` を Maya 2023／V-Ray 5.20.02 と Maya 2026／V-Ray 7.40.04 の `mayapy` で実行。球面ドームの新規作成、既存ドームへの再接続、HDR 差し替え時のビットマップキャッシュのクリア、シーン切り替え時の監視終了を確認した。Y-up の Maya 2023／V-Ray 5 CPU レンダーでは ±X・±Y・±Z の6方向を色で照合し、同じパスの HDR を差し替えると背景が赤から緑に変わった。
- **ビルドと静的検査**。V-Ray 同期追加後に Rust 全48テスト、Clippy、整形チェック、リリースビルドが成功。初期の `scripts/build-native.ps1` と `cargo build --release` も成功した。
- **Alembic とカメラ**。Maya 2026 の `mayapy` で `tests/maya_integration.py` を実行し、標準 AbcExport（Alembic 1.8.5）の出力を DLL（1.8.9）で読めることを確認。非対称フラスタム、階層、非一様スケール、法線生成、カメラ、無効ファイル、日本語パス、繰り返しロードを検証した。Maya 依存テストでは Film Fit 4モード、オフセット、スクイーズ、ロール、正投影、非正方形ピクセルを確認した。
- **実レンダーとの画角比較**。Maya Software の 640×360 レンダーと Rust 投影を Film Fit 4モードの3色マーカー計12点で比較。誤差は約0.015〜0.255ピクセルで、基準の1.5ピクセル以内。実画面で ABC とカメラ JSON の読み込み、GPU 表示、再読み込み、ドラッグ・スクロール時のカメラ固定を確認した。
- **フィルムオフセットの保持**。コードを確認し、Maya のレンダーフラスタムに含まれる水平・垂直のオフセットが `.camera.json` の `projection` に入り、Rust の 3D プレビューで使われることを確認した。提示された `/persp/perspShape` の JSON は対称の透視投影。元の Maya 属性値と制作シーンでのレンダー画角は未確認。
- **選択エクスポート**。Maya 2026 の `tests/maya_selection.py` と Rust の `selected_export_excludes_unselected_siblings` で単体・グループ選択、選択復元、空選択エラー、未選択の兄弟メッシュとカメラ親配下の未選択メッシュの除外、親 Transform とカメラ座標を確認。階層一覧削除後のリリース画面でも画角を維持した。
- **HDR の数値処理と表示**。編集機能追加時の `cargo test -- --include-ignored` は全12件成功。EV、回転、継ぎ目、ライト OFF、一定色環境光のエネルギー、1.0超の HDR 値の往復、EXR のリニア値、非2:1画像の拒否を検証した。実画面で既定照明を切るとモデルが黒くなり、ソフトボックスを追加するとその光で見えることを確認した。
- **既定照明の切り替え**。実画面で起動時にオン、ライト追加時にオフ、Ctrl+Z でゼロに戻すとオンになることを確認。最後のルートレイヤーの削除・取り消し・やり直しと、子レイヤーだけを削除した場合の状態は自動テストで確認した。この時点で全47テスト、Clippy、整形チェック、リリースビルドが成功した。
- **編集画面**。形状変更、芯の移動、左右の切り落とし、`Hard cut` の角度変更、極付近の 3D 投影、左右の継ぎ目、`Flat` への切り替えを確認。Falloff のプリセット変更と点の追加・移動・削除、両キャンバスでのドラッグ、無効ライトの画像・輪郭の非表示と再表示も確認した。
- **Falloff の数値検証**。円形 `Linear` の半径50%は8bitで128付近。矩形 `Linear` は芯を動かしても輪郭で明るさが揃い、対角線をまたぐ輝度の傾きが連続する。`Gradient` の横・縦の直線変化と、`Box Gradient` の辺・対角線・中心軸付近、横・縦の独立カーブを検証した。修正段階で全21テスト、Clippy、リリースビルドが成功し、実画面で `Linear` の見え方とリンク解除時の2本のカーブを確認した。通常版の exe が起動中で更新できなかった時点の修正版は `target/rectangle-fix/release/HDR_STUDIO.exe`。
- **参照画像の調査**。円形 `Linear` の半径50%における画面上の緑チャンネルは、中心比で HDR Light Studio が約52%、HDR STUDIO が約92%。HDR STUDIO はリニア HDR 値を減衰させ、キャンバス表示に `x / (1 + x)` と sRGB 変換を適用する。

### 未確認

- Maya 再起動後のシェルフ復元、Maya 2026 の GUI でのシェルフ起動、PySide ウィンドウを経由した実ファイルの同期開始・停止。
- Maya の GUI で実行中の V-Ray IPR の自動再描画、制作シーンでの 700 × 350 px HDR の同期時間、V-Ray GPU と Z-up の方位。
- 制作シーンでの画角、Maya 2026 以外での Alembic・カメラ検証、Arnold 等の画角、特殊カメラ・レンズ歪み、巨大座標の精度、大規模シーンの表示速度。小規模テストの読み込み時間は、そのテスト条件に限る。
- 制作 HDR 素材、巨大 HDR／EXR、色空間変換、他レンダラーとの照明結果の一致。Maya Software との比較で検証したのは画角。
- HDR Light Studio の表示 LUT と元の HDR 値、表示設定とライト寸法を揃えた場合の画素単位の一致。
- 3D 投影、Falloff、カットを編集した結果の UI 書き出しと外部レンダラーでの見え方。最後のルートレイヤーを UI から削除したときの表示。
- 提供資料の省略部分、会話画面に表示されていないログ、退避ファイルの内容。今回の記録の退避先は `C:/Users/55/AppData/Local/nvim-data/codex-progress/22996-5113926466000.archive.json`。前回の退避先は `C:/Users/55/AppData/Local/nvim-data/codex-progress/30696-3473256645000.archive.json`。
