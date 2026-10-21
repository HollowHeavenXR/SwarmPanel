{ lib
, python3Packages
, wrapGAppsHook4
, gobject-introspection
, gtk4
, libadwaita
, libsecret
, glib
, adwaita-icon-theme
, desktop-file-utils
}:

python3Packages.buildPythonApplication {
  pname = "swarmpanel";
  version = "1.0.0";
  pyproject = true;
  src = lib.cleanSource ../.;

  build-system = [ python3Packages.setuptools ];

  nativeBuildInputs = [
    wrapGAppsHook4
    # Provides the typelib search path the PyGObject `gi.require_version`
    # calls need at runtime, not just at build time.
    gobject-introspection
    desktop-file-utils
  ];

  buildInputs = [
    gtk4
    libadwaita
    # Secret Service client, for the session token. The app degrades to a
    # 0600 file when no secret service is running, so this is a soft
    # dependency -- but shipping it means the good path works by default.
    libsecret
    glib
    adwaita-icon-theme
  ];

  dependencies = with python3Packages; [
    pygobject3
    websockets
  ];

  # buildPythonApplication's own wrapper would run before wrapGAppsHook4 and
  # drop the GI/GSettings environment; deferring it lets the GApps hook fold
  # its variables into the single final wrapper.
  dontWrapGApps = true;
  preFixup = ''
    makeWrapperArgs+=("''${gappsWrapperArgs[@]}")
  '';

  postInstall = ''
    install -Dm644 data/studio.xenusanimations.SwarmPanel.desktop \
      $out/share/applications/studio.xenusanimations.SwarmPanel.desktop
    install -Dm644 data/studio.xenusanimations.SwarmPanel.metainfo.xml \
      $out/share/metainfo/studio.xenusanimations.SwarmPanel.metainfo.xml
    install -Dm644 data/icons/studio.xenusanimations.SwarmPanel-256.png \
      $out/share/icons/hicolor/256x256/apps/studio.xenusanimations.SwarmPanel.png
  '';

  doCheck = true;
  nativeCheckInputs = [ desktop-file-utils ];
  checkPhase = ''
    runHook preCheck
    desktop-file-validate data/studio.xenusanimations.SwarmPanel.desktop
    # Byte-compile every module: catches syntax errors in screens the smoke
    # test might not have reached.
    ${python3Packages.python.interpreter} -m compileall -q swarmpanel
    runHook postCheck
  '';

  # The GI bindings are resolved at import time, so a successful import is a
  # real check that the wrapper's typelib path is correct.
  pythonImportsCheck = [ "swarmpanel" "swarmpanel.core.api" "swarmpanel.core.nav" ];

  meta = with lib; {
    description = "Native Linux client for SwarmPanel, the Aria music bot fleet command centre";
    homepage = "https://swarmpanel.xenusanimations.studio";
    license = licenses.unfree;
    platforms = platforms.linux;
    mainProgram = "swarmpanel";
  };
}
