# HDR STUDIO

Rust 製ライティングツールの初期実装です。Alembic（Ogawa `.abc`）を C++ DLL で読み、Rust / OpenGL で固定カメラの 3D プレビューを表示します。

## ビルド・起動（Windows x64）

必要なもの: Rust MSVC、Visual Studio の C++ ビルドツールと Windows SDK、CMake 3.24 以降、Git。

```powershell
./scripts/build-native.ps1
cargo run -- tests/fixtures/basic.abc
```

初回のネイティブビルドで公式 Alembic 1.8.9 と Imath 3.1.12 を `native/vendor` に取得します。両ライブラリは専用の `hdr_abc.dll` に静的リンクし、DLL を `target/debug` と `target/release` に配置します。実行時に Maya のインストールは必要ありません。ライセンス文書は `native/build/licenses` にコピーします。

```powershell
cargo build --release
./target/release/HDR_STUDIO.exe
./target/release/HDR_STUDIO.exe C:/temp/lighting.abc
```

**Import Alembic…** またはドラッグ＆ドロップで開きます。右側の **Fixed camera** タブでカメラを選び、読み込み情報を確認できます。画枠はウィンドウの大きさに合わせて拡縮し、アスペクト比を維持します。カメラがない場合はカメラの読み込みを促します。

## プロジェクトの保存・読み込み

- **File → New Project** で空のプロジェクトを開始します。
- **File → Save Project…** で現在のプロジェクトを `.lmo` に保存します。初回は `NewProject.lmo`、以降は開いているプロジェクト名が入力されます。ダイアログで名前と保存先を変更できます。
- **File → Load Project…** または `.lmo` のドラッグ＆ドロップで開きます。ライトレイヤー、HDR／EXR の画像データ、取り込んだ Alembic、カメラの画角と選択状態をファイル内に保存します。元の `.abc` や `.camera.json`、HDR／EXR が移動しても復元できます。

## HDR の編集と照明

1. **Import Alembic…** でモデルとカメラを読み込みます。
2. ライトレイヤーがゼロのときは **Default lighting** が自動でオンになり、内蔵のキーライトとベースの明るさでモデルを表示します。レイヤーがあるときは **Lighting** タブで設定できます。**HDR lighting** は環境光・映り込みをまとめて ON/OFF します。
3. **HDR environment → + Add light → Shape → Image → Load HDR / EXR…** で、2:1 の全天球パノラマをレイヤーとして読み込みます。HDR/EXR をウィンドウにドロップしても Image レイヤーを追加できます。HDR キャンバスのダブルクリックでは丸形ライトを追加します。レイヤーを追加するたびに **Default lighting** が自動で OFF になります。
4. キャンバスのライトをドラッグして移動します。ライトを選ぶと、色・強度・幅・高さ・減衰カーブ・経度・緯度を編集できます。ライトごとに ON/OFF、削除ができます。
5. タブ上部の `Exposure (EV)` と `Rotation (°)` は合成全体に適用します。Image レイヤーの `Image rotation (°)` は水平方向のオフセット、`Image exposure (EV)` は画像の露出、`Alpha` はレイヤーの不透明度です。`Linear` の既定はオンで、オフにすると入力を sRGB としてリニア RGB に変換します。
6. `Export HDR…` で照明を合成した Radiance `.hdr` を保存します。指定した出力解像度を使い、1.0 を超える値を保持します。表示用のトーンマッピング、Default lighting、プレビュー材質は書き出しません。

レイヤーは一覧の下から上へ合成します。行の左端にあるハンドルをドラッグすると順序が変わり、行の右側で **Blend** を切り替えられます。鍵アイコンをオンにすると、レイヤーの選択、プロパティの変更、ライトの移動がロックされます。ロック中も合成と並べ替えは有効です。

レイヤー行を右クリックすると **Rename…**、**Solo / Exit Solo**、**Delete** が使えます。ソロの行は琥珀色になり、そのレイヤーをプレビューで単独確認できます。解除すると元の表示に戻ります。HDR 書き出しは通常のレイヤー設定を使います。選択中のレイヤーは **Delete** キーで削除できます。位置を持つライトは **矢印キー** で 1° ずつ移動できます。**Ctrl+Z** で HDR 編集を取り消し、**Ctrl+Y** でやり直せます。

Image の既定の Blend は **Over** で、Alpha に応じて下の合成結果を覆います。**Add** はライトの RGB を加算、**Multiply** は照射範囲内で下の RGB にライトの RGB を乗算、**Amplify** は下の RGB に `1 + ライトの RGB` を乗算します。Multiply と Amplify を使うときは、下のレイヤーに光が必要です。Multiply の **Mask** をオンにすると、ライトの範囲外が黒になります。

**Shape → CompositeLight → Edit composite** で子レイヤーの一覧とキャンバスを開きます。**+ Add light** で子レイヤーを追加し、**Back** で親の一覧に戻ります。親を選び、HDR Canvas 上の合成結果または十字ハンドルをドラッグすると、子ライト全体が移動します。経度・緯度の数値入力でも配置できます。子レイヤーを合成した結果に親の **Blend** を適用します。編集中も 3D プレビューと HDR 書き出しには全体の合成結果を使います。

**Over** は減衰カーブを不透明度として使い、下の色をライトの色で覆います。**Low Pass** はライトの強度と色で決まる上限を下の明るさに適用し、減衰カーブで効果を混ぜます。**Invert** をオンにすると、Add は下の色比を保ったまま明るさを引く Dark Light になり、Low Pass は明るさの下限になります。Amplify では増光倍率の逆数で減光します。

**Shape** は **Layer properties** の名前欄の直下にあるアイコン付きプルダウンです。**Mapping, hotspot & cut → Mapping** の既定値は **3D** です。キャンバスの上端・下端へライトを移動すると、全天球への投影に合わせて横に広がり、輪郭や切断線が曲がります。左右の端をまたぐライトは両側に続きます。**Cutoff rotation** はライト面上の角度です。**Flat** を選ぶとキャンバス上で形を保つ従来の平面配置になります。

**Falloff curve** はライトの芯（左端）から外周（右端）までの明るさを調整するトーンカーブです。新規ライトは **Round**、減衰は **Smooth**、幅40°、高さ40°です。**Softbox / Linear / Tight / Hard edge** も選べます。点をドラッグして動かし、グラフのダブルクリックで追加、点の右クリックで削除できます。両端の明るさは縦方向に調整できます。

**Shape → Rectangle** の **Box Gradient** は、芯から各辺へ広がる横・縦のカーブを掛け合わせます。**Preset → Linear** は直線補間を使用します。**Interpolation → Smooth** はアンカーを通り、途中で明るさが同じ方向に変化し続ける区間では傾きを滑らかにつなぎます。芯と外周で傾きをゼロにします。**Link horizontal and vertical** を外すと、水平・垂直のカーブと補間を個別に設定できます。

**Shape → Gradient** は片側から反対側へ明るさが変わる矩形です。既定の **Linear** は左から右へ暗くなり、**Gradient direction** で方向を変えられます。**Gradient curve** で始点から終点までを編集できます。キャンバス上で矩形の外形を保つには **Mapping → Flat** を選びます。

キャンバス上部の **HDR Canvas** は合成結果にトーンマッピングと sRGB 変換をかけて表示します。**Falloff Preview** は選択中のライトの芯を白の基準にして、減衰をグレースケールで直接表示します。Gradient ではカーブの最大値を白の基準にします。どちらの表示でも見えているライトをドラッグして移動できます。レイヤーのチェックを外すと両方の表示から消えます。3D 照明と書き出しには合成済みのリニア HDR を使います。

ツールバーの **3D Preview** をオフにすると、3D の描画と描画用データの準備を止め、キャンバスを 2:1 のまま空いた領域に拡大します。キャンバスでの編集と HDR の書き出しは引き続き使えます。再びオンにすると、現在の HDR 合成結果で 3D プレビューを再開します。

**Shape → Round** は幅と高さを変えると楕円になります。Rectangle と Round では **Hotspot position** の点をドラッグすると、外形を固定したまま明るさの芯が移動します。**Hard cut** をオンにすると、円形ダイヤルか **Cutoff rotation** の数値入力で、切り落とす方向を 0〜360° から指定できます。矢印は切る側を指し、0° は右、90° は下です。**Trim** はその方向に沿った切断線の位置を調整します。50% でライトの中央を通ります。

`HDR background` は固定カメラから見た環境背景を表示します。`Preview material` の Roughness / Metallic で映り込みを確認できます。メッシュの元の材質を再現する機能ではありません。カメラの画角は照明編集によって変えません。

読み込みと書き出しは別スレッドで処理します。インタラクティブな照明確認には 512×256 の HDR テクスチャ、SH による拡散光、ミップマップによる概算の反射を使用します。**Lighting → Preview material → Ray-traced reflections** をオンにすると、滑らかな面に別の面や物体が映り込みます。反射は1段で、粗い面では HDR の近似反射に滑らかに戻ります。書き出しは元の HDR 画像とライト定義から指定解像度で再合成します。

**Lighting → HDR soft shadows** はモデル表面に、**Show floor shadows** はモデルの最下部に合わせた床に HDR の影を表示します。両方とも **HDR lighting** がオンのときに使えます。HDRI 上で光源の見かけの幅が広いほど、影の縁は柔らかくなります。床と表面の影はプレビュー用です。

3D プレビューは専用スレッドの OpenGL で描画します。**Lighting** の `Min shadow samples` と `Min preview resolution` は操作中の値で、既定は4サンプル・50%です。アイドル時は `Idle shadow samples` と `Idle preview resolution` の上限まで段階的に上がり、操作を再開すると最小値に戻ります。上限の既定は32サンプル・100%です。影サンプル数は表面と床に共通で、解像度の倍率は3D表示枠の物理ピクセル数に対して適用します。現在表示している品質は Lighting タブの `Preview` 欄に表示します。影のサンプルを画素ごとに変えるため、半影は静止時にもちらつかない粒状になります。

Lighting の設定は `%LOCALAPPDATA%/HDR STUDIO/settings.json` に保存し、次回起動時に復元します。ライトレイヤーが空の起動時は **Default lighting** がオンです。HDR lighting、HDR background、HDR soft shadows の既定値もオンです。

影と反射のレイ追跡データは複数のテクスチャページに分けて GPU に送ります。扱えるシーンの大きさは GPU メモリ、テクスチャ配列の容量、PC のメモリに依存します。転送できない場合はステータス欄に原因を表示します。

HDR/EXR は既定でリニア RGB として読み込みます。OCIO / ACES の変換は未対応で、EXR の負値は発光量として 0 にします。**Export HDR…** の出力は合成済み HDR 画像です。

## モデル上でライトを配置・移動する

3D プレビュー上部の **Light placement** でモードを選び、モデルの表面をクリックするか、マウスの左ボタンを押したまま表面をドラッグします。

- **Reflection** はその位置に映り込む方向にライトを配置します。初期モードです。
- **Diffuse** はポインター位置の表面の法線に沿ってライトを配置します。

有効なライトが選択されていれば移動し、選択がなければ新しく作成します。操作開始時に **Shift** を押していると新規ライトを1個追加します。ドラッグ中は HDR Canvas と 3D 照明が追従します。ポインターがモデルから外れると最後に当たった位置を保ちます。

## Maya のシェルフに登録する

Maya の Script Editor の **Python** タブで次を実行します。`HDRStudio` シェルフに **HDR STUDIO** ボタンが追加されます。旧ボタンを登録済みの場合は、Maya を起動し直して次を実行すると既存のボタンが更新されます。

```python
import sys
sys.path.insert(0, r"C:/Users/55/Desktop/WORK/Programs/rust/HDR_STUDIO/scripts")
import maya_hdr_studio
maya_hdr_studio.install_shelf()
```

ボタンは登録時の `scripts` フォルダーにある `maya_hdr_studio.py` と `hdr_studio_maya.png` を参照します。フォルダーを移動した場合はパスを変更して登録し直します。

## Maya の V-Ray Dome Light と同期する

1. HDR STUDIO の **HDR environment → Link Maya V-Ray** を押すと、`%LOCALAPPDATA%\HDR STUDIO\vray-sync\hdr_studio_vray.hdr` に 700 × 350 px の同期用 HDR が作られます。
2. 同じPC・同じWindowsユーザーで Maya を起動し、`HDRStudio` シェルフの **HDR STUDIO → V-Ray Sync → Start Sync** を押します。Maya は同期用の HDR を同じ場所から自動で参照します。
3. Maya はシーン全体の V-Ray Dome Light を確認します。0灯なら `HDRStudioDome` を作り、1灯なら使用するか確認します。**Use Existing** を選ぶと既存の **Dome Tex** 接続を切り替え、**Use Dome Tex** と **Dome Spherical** を有効にします。**Cancel** を選ぶと同期開始を取り消します。2灯以上ある場合は警告を表示し、同期開始を取り消します。

HDR STUDIO は編集操作中も全レイヤーの合成結果を同期用ファイルへ書き出します。**V-Ray HDR saved** の括弧内に保存回数を表示し、ファイルが一時的に使用中なら保存を再試行します。Maya は同じフォルダーで `*.maya-…-a.hdr` と `*.maya-…-b.hdr` を交互に更新し、保存完了後にドームのテクスチャパスを切り替えます。最後の HDR 更新から **Reload delay**（250〜2000 ms、初期値500 ms）で指定した時間が経過すると、File ノードの **Reload** を1回実行し、V-Ray IPR に再描画を要求します。スライダーは同期中も調整でき、待機中に調整すると新しい待ち時間でタイマーが始まります。

**Sync now** で手動更新、**Stop V-Ray sync** で HDR STUDIO 側の書き出しを停止できます。Maya 側の再読み込みは **Stop Sync** で停止します。ウィンドウを閉じても再読み込みは続きます。同期を止めた後も、Maya のドームは最後に適用した HDR を参照します。Maya でシーンを開き直したら **Start Sync** を押し、HDR STUDIO を再起動したら **Link Maya V-Ray** を押します。

### 仕上げ用 HDR を V-Ray に適用する

HDR STUDIO の **HDR environment** で出力解像度を選び、**Render HDR for Maya…** を押して仕上げ用 `.hdr` の保存先を指定します。V-Ray の球面配置に合わせて保存し、完了後に Maya の Dome Light が指定先を直接参照します。編集中のプレビューに戻すときは **Resume Live Sync** を押します。

同期専用ファイルは Maya の Y-up／V-Ray の球面配置に合わせた向きで保存します。既存ドームに独自のマッピング回転や Finite Dome の設定がある場合は、V-Ray 側の結果にもその設定が反映されます。通常の HDR は **Export HDR…** で書き出せます。

## Maya のレンダー画角を渡す

**画角を合わせる場合は、付属の Maya スクリプトで `.abc` と `.camera.json` をセットで出力してください。** ABC 単体のプレビューはアーカイブのフィルムゲートとして扱い、Maya の最終レンダー画枠と一致するとは表示しません。

`HDRStudio` シェルフの **HDR STUDIO → Alembic Export** を開きます。Outliner でモデルを選び、**Scope → Selected objects** と **Camera** を指定して **Export Alembic…** を押します。シーン全体のメッシュを対象にする場合は **Scene meshes** を選びます。**Renderable cameras** ではレンダリングが有効なカメラを出力します。

Script Editor からも実行できます。

```python
import sys
sys.path.insert(0, r'C:/Users/55/Desktop/WORK/Programs/rust/HDR_STUDIO/scripts')
import maya_hdr_studio
maya_hdr_studio.export(r'C:/temp/lighting.abc', camera='renderCam')
```

`camera` にはカメラの Transform または Shape 名を指定します。同名が複数ある場合はフル DAG パスを使います。省略すると renderable なカメラを出力します。モデルはメッシュを含むルート階層を出力します。対象を絞る場合:

```python
maya_hdr_studio.export(r'C:/temp/lighting.abc', camera='|shot|renderCam', roots=['|product'])
```

選択したモデルだけを出力する場合は、Maya でモデルまたはグループを選択してから実行します。グループの子孫も含みます。未選択の兄弟モデルは含めず、指定カメラと必要な親 Transform は自動で追加します。実行後は元の選択を復元します。未選択時はエラーにします。

```python
maya_hdr_studio.export_selected(r'C:/temp/lighting.abc', camera='persp')
```

現在のフレームだけを書き出します。Maya の内部単位（cm）を保持します。補助 JSON はレンダー解像度・ピクセルアスペクト・Maya API のレンダーフラスタム・ポスト投影変換・カメラのワールド逆行列を含みます。Film Fit、フィルムオフセット、レンズスクイーズ、フィルムロール、正投影を Rust 側で推測し直すことを避けます。

JSON がある場合はそのサンプル時刻を自動使用します。ファイルサイズ・更新時刻・サンプル時刻・カメラパスが一致しない JSON はエラーにします。ABC と JSON は一緒に再出力してください。

## 確認

```powershell
cargo test
& 'C:/Program Files/Autodesk/Maya2026/bin/mayapy.exe' tests/maya_integration.py
& 'C:/Program Files/Autodesk/Maya2026/bin/mayapy.exe' tests/maya_selection.py
cargo test -- --ignored --nocapture
cargo run -- --inspect tests/output/maya_fit_0.abc
```

通常のテストは DLL と同梱の小さな ABC フィクスチャを使います。Maya を必要とするテストは `ignored` に分けています。Maya 用スクリプトは独立したスタンドアロンプロセスでテスト用シーンを作成し、`tests/output` に出力します。開いている Maya のシーンは操作しません。

## 対応範囲

- Ogawa のポリゴンメッシュ、階層、Transform（継承リセットを含む）、カメラ、可視性。
- 凹形状を含む n-gon の三角形化。ABC に法線があれば保持し、省略されていれば共有頂点の周囲の面からスムース法線を生成。3D 表示と Reflection／Diffuse のライト配置に同じ法線を使用。
- 静止サンプル表示。秒指定で再読み込み可能。アニメーションの補間・再生は未実装。
- HDR/EXR の読み込み、HDR キャンバスでのソフトボックス編集、露出／回転、環境光と映り込みのプレビュー、HDR 書き出し。内蔵照明の ON/OFF。
- Maya の V-Ray Dome Light への HDR 同期。元のマテリアル・テクスチャと Maya からのモデル・カメラのリアルタイム転送は未実装。
- HDF5 の古い ABC、SubD の細分化、カーブ、ポイント、レンズ歪み、DOF、モーションブラーは未対応。非対応オブジェクトは Import notes に表示。
- 任意のレンダラー／特殊なカメラプラグインまでの画角一致は未確認。Maya Software の参照画像による検証結果は `PROJECT_PROGRESS.md` に記録。

## 構成

- `native/bridge.*`: C ABI を公開する DLL。例外を DLL 内で捕捉し、DLL 内のメモリは DLL 側で解放。
- `src/abc.rs`: DLL 呼び出しと所有権の管理。
- `src/scene.rs`: 階層、ワールド座標化、三角形化、補助 JSON の整合性確認。
- `src/camera.rs`: 投影行列と画枠。
- `src/renderer.rs`: GPU 描画と深度テスト。
- `src/environment.rs`: HDR 画像、ライト合成、HDR 書き出し、環境光の SH 係数。
- `src/hdr_editor.rs`: HDR 編集 UI とキャンバス、非同期読み込み／書き出し。
- `scripts/maya_hdr_studio.py`: Maya のシェルフ登録、Alembic とカメラの出力、V-Ray 用 HDR の同期。
- `scripts/hdr_studio_maya.png`: Maya シェルフ用の 32 × 32 px PNG。
- `src/vray_link.rs`: V-Ray 同期用 HDR の書き出し。
# HDR_Studio
# HDR_Studio
