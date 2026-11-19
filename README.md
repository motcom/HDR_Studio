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

`Open Alembic…` またはドラッグ＆ドロップで開きます。右側の **Lighting** タブ内の **Fixed camera** でカメラを選び、読み込み情報を確認できます。画枠はウィンドウの大きさに合わせて拡縮し、アスペクト比を維持します。カメラがない場合はカメラの読み込みを促します。

## HDR の編集と照明

1. `Open Alembic…` でモデルとカメラを読み込みます。
2. 右側の **Lighting** タブで **Default lighting** を OFF にすると、内蔵のキーライトとベースの明るさが消えます。**HDR lighting** は環境光・映り込みをまとめて ON/OFF します。
3. **HDR environment** タブの `Load HDR / EXR…` で 2:1 の全天球パノラマを読み込むか、黒い環境から `+ Add light` でライトを追加します。HDR キャンバスのダブルクリックでも追加できます。
4. キャンバスのライトをドラッグして移動します。ライトを選ぶと、色・強度・幅・高さ・減衰カーブ・経度・緯度を編集できます。ライトごとに ON/OFF、削除ができます。
5. `Exposure (EV)` と `Rotation (°)` は、ベース画像と追加ライトの両方に適用します。
6. `Export HDR…` で照明を合成した Radiance `.hdr` を保存します。指定した出力解像度を使い、1.0 を超える値を保持します。表示用のトーンマッピング、Default lighting、プレビュー材質は書き出しません。

ライトは一覧の下から上へ合成し、矢印ボタンで順序を変更できます。**Add** はライトの RGB を加算、**Multiply** は照射範囲内で下の RGB にライトの RGB を乗算、**Amplify** は下の RGB に `1 + ライトの RGB` を乗算します。Multiply と Amplify を使うときは、ベース画像か下の Add レイヤーに光が必要です。

**Shape** は **Layer properties** の名前欄の直下にあります。**Mapping, hotspot & cut → Mapping** の既定値は **3D** です。キャンバスの上端・下端へライトを移動すると、全天球への投影に合わせて横に広がり、輪郭や切断線が曲がります。左右の端をまたぐライトは両側に続きます。**Cutoff rotation** はライト面上の角度です。**Flat** を選ぶとキャンバス上で形を保つ従来の平面配置になります。

**Falloff curve** はライトの芯（左端）から外周（右端）までの明るさを調整するトーンカーブです。新規ライトの既定は **Smooth**、幅40°、高さ40°です。**Softbox / Linear / Tight / Hard edge** も選べます。点をドラッグして動かし、グラフのダブルクリックで追加、点の右クリックで削除できます。両端の明るさは縦方向に調整できます。

**Shape → Rectangle** の **Box Gradient** は、芯から各辺へ広がる横・縦のカーブを掛け合わせます。**Preset → Linear** は直線補間を使用します。**Interpolation → Smooth** はアンカーを通り、途中で明るさが同じ方向に変化し続ける区間では傾きを滑らかにつなぎます。芯と外周で傾きをゼロにします。**Link horizontal and vertical** を外すと、水平・垂直のカーブと補間を個別に設定できます。

**Shape → Gradient** は片側から反対側へ明るさが変わる矩形です。既定の **Linear** は左から右へ暗くなり、**Gradient direction** で方向を変えられます。**Gradient curve** で始点から終点までを編集できます。キャンバス上で矩形の外形を保つには **Mapping → Flat** を選びます。

キャンバス上部の **HDR Canvas** は合成結果にトーンマッピングと sRGB 変換をかけて表示します。**Falloff Preview** は選択中のライトの芯を白の基準にして、減衰をグレースケールで直接表示します。Gradient ではカーブの最大値を白の基準にします。どちらの表示でも見えているライトをドラッグして移動できます。レイヤーのチェックを外すと両方の表示から消えます。3D 照明と書き出しには合成済みのリニア HDR を使います。

**Shape → Round** は幅と高さを変えると楕円になります。Rectangle と Round では **Hotspot position** の点をドラッグすると、外形を固定したまま明るさの芯が移動します。**Hard cut** をオンにすると、円形ダイヤルか **Cutoff rotation** の数値入力で、切り落とす方向を 0〜360° から指定できます。矢印は切る側を指し、0° は右、90° は下です。**Trim** はその方向に沿った切断線の位置を調整します。50% でライトの中央を通ります。

`HDR background` は固定カメラから見た環境背景を表示します。`Preview material` の Roughness / Metallic で映り込みを確認できます。メッシュの元の材質を再現する機能ではありません。カメラの画角は照明編集によって変えません。

読み込みと書き出しは別スレッドで処理します。インタラクティブな照明確認には 512×256 の HDR テクスチャ、SH による拡散光、ミップマップによる概算の反射を使用します。書き出しは元の HDR 画像とライト定義から指定解像度で再合成します。プレビューはレイトレーシングではなく、シャドウ／遮蔽と正確な粗さの積分は未対応です。

HDR/EXR はリニア RGB 前提です。OCIO / ACES の変換は未対応で、EXR の負値は発光量として 0 にします。出力は合成済み HDR 画像です。ライトの編集状態を別プロジェクトとして保存する機能、Undo/Redo は未実装です。

## モデル上でライトを配置・移動する

3D プレビュー上部の **Light placement** でモードを選び、モデルの表面をクリックするか、マウスの左ボタンを押したまま表面をドラッグします。

- **Reflection** はその位置に映り込む方向にライトを配置します。
- **Diffuse** はポインター位置の表面の法線に沿ってライトを配置します。

有効なライトが選択されていれば移動し、選択がなければ新しく作成します。操作開始時に **Shift** を押していると新規ライトを1個追加します。ドラッグ中は HDR Canvas と 3D 照明が追従します。ポインターがモデルから外れると最後に当たった位置を保ちます。

## Maya のレンダー画角を渡す

**画角を合わせる場合は、付属の Maya スクリプトで `.abc` と `.camera.json` をセットで出力してください。** ABC 単体のプレビューはアーカイブのフィルムゲートとして扱い、Maya の最終レンダー画枠と一致するとは表示しません。

Maya の Script Editor の Python タブで実行:

```python
import sys
sys.path.insert(0, r'C:/Users/55/Desktop/WORK/Programs/rust/HDR_STUDIO/scripts')
import maya_export
maya_export.export(r'C:/temp/lighting.abc', camera='renderCam')
```

`camera` にはカメラの Transform または Shape 名を指定します。同名が複数ある場合はフル DAG パスを使います。省略すると renderable なカメラを出力します。モデルはメッシュを含むルート階層を出力します。対象を絞る場合:

```python
maya_export.export(r'C:/temp/lighting.abc', camera='|shot|renderCam', roots=['|product'])
```

選択したモデルだけを出力する場合は、Maya でモデルまたはグループを選択してから実行します。グループの子孫も含みます。未選択の兄弟モデルは含めず、指定カメラと必要な親 Transform は自動で追加します。実行後は元の選択を復元します。未選択時はエラーにします。

```python
import importlib
importlib.reload(maya_export)  # すでに読み込み済みの場合も変更を反映
maya_export.export_selected(r'C:/temp/lighting.abc', camera='persp')
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
- 元のマテリアル・テクスチャ、Maya とのライブ接続は未実装。
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
- `scripts/maya_export.py`: Maya からの ABC とカメラ情報の出力。
# HDR_Studio
# HDR_Studio
