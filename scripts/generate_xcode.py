#!/usr/bin/env python3
"""Regenerate the dependency-free, shared iOS/macOS Xcode project."""
import hashlib
from pathlib import Path
import plistlib

root = Path(__file__).resolve().parent.parent / "Apple"
project = root / "Torrbaby.xcodeproj"
project.mkdir(exist_ok=True)
objects = {}
def oid(name):
    return hashlib.sha1(name.encode()).hexdigest()[:24].upper()
def obj(key_name, isa, **kwargs):
    key = oid(key_name)
    objects[key] = dict(isa=isa, **kwargs)
    return key
sources = []
refs = []
for file in sorted((root / "Torrbaby").glob("*.swift")):
    ref = obj(file.name, "PBXFileReference", lastKnownFileType="sourcecode.swift", path=file.name, sourceTree="<group>")
    refs.append(ref)
    sources.append(obj("build-" + file.name, "PBXBuildFile", fileRef=ref))
assets = obj("assets", "PBXFileReference", lastKnownFileType="folder.assetcatalog", path="Assets.xcassets", sourceTree="<group>")
asset_build = obj("assets-build", "PBXBuildFile", fileRef=assets)
refs.append(assets)
appgroup = obj("appgroup", "PBXGroup", children=refs, path="Torrbaby", sourceTree="<group>")
product = obj("product", "PBXFileReference", explicitFileType="wrapper.application", includeInIndex=0, path="Torrbaby.app", sourceTree="BUILT_PRODUCTS_DIR")
products = obj("products", "PBXGroup", children=[product], name="Products", sourceTree="<group>")
main = obj("main", "PBXGroup", children=[appgroup, products], sourceTree="<group>")
srcphase = obj("sources", "PBXSourcesBuildPhase", buildActionMask=2147483647, files=sources, runOnlyForDeploymentPostprocessing=0)
resphase = obj("resources", "PBXResourcesBuildPhase", buildActionMask=2147483647, files=[asset_build], runOnlyForDeploymentPostprocessing=0)
frameworks = obj("frameworks", "PBXFrameworksBuildPhase", buildActionMask=2147483647, files=[], runOnlyForDeploymentPostprocessing=0)
shared = {"PRODUCT_NAME": "$(TARGET_NAME)", "PRODUCT_BUNDLE_IDENTIFIER": "org.torrbaby.app", "SWIFT_VERSION": "5.0", "SDKROOT": "auto", "SUPPORTED_PLATFORMS": "iphoneos iphonesimulator macosx", "MACOSX_DEPLOYMENT_TARGET": "15.0", "IPHONEOS_DEPLOYMENT_TARGET": "18.0", "TARGETED_DEVICE_FAMILY": "1,2", "CODE_SIGN_STYLE": "Automatic", "GENERATE_INFOPLIST_FILE": "NO", "INFOPLIST_FILE": "Torrbaby/Info.plist", "CURRENT_PROJECT_VERSION": "1", "MARKETING_VERSION": "0.1.0", "ASSETCATALOG_COMPILER_APPICON_NAME": "AppIcon", "ENABLE_HARDENED_RUNTIME": "YES", "SWIFT_EMIT_LOC_STRINGS": "YES", "COMBINE_HIDPI_IMAGES": "YES"}
configs = []
projectconfigs = []
for mode in ("Debug", "Release"):
    setting = dict(shared, SWIFT_OPTIMIZATION_LEVEL="-Onone" if mode == "Debug" else "-O", DEBUG_INFORMATION_FORMAT="dwarf" if mode == "Debug" else "dwarf-with-dsym")
    if mode == "Debug": setting["SWIFT_ACTIVE_COMPILATION_CONDITIONS"] = "DEBUG"
    configs.append(obj("target-" + mode, "XCBuildConfiguration", buildSettings=setting, name=mode))
    projectconfigs.append(obj("project-" + mode, "XCBuildConfiguration", buildSettings={"CLANG_ENABLE_MODULES": "YES", "CLANG_ENABLE_OBJC_ARC": "YES"}, name=mode))
targetconfig = obj("targetconfig", "XCConfigurationList", buildConfigurations=configs, defaultConfigurationIsVisible=0, defaultConfigurationName="Release")
projectconfig = obj("projectconfig", "XCConfigurationList", buildConfigurations=projectconfigs, defaultConfigurationIsVisible=0, defaultConfigurationName="Release")
target = obj("target", "PBXNativeTarget", buildConfigurationList=targetconfig, buildPhases=[srcphase, frameworks, resphase], buildRules=[], dependencies=[], name="Torrbaby", productName="Torrbaby", productReference=product, productType="com.apple.product-type.application")
rootobj = obj("project", "PBXProject", attributes={"LastUpgradeCheck": "2700", "BuildIndependentTargetsInParallel": "YES"}, buildConfigurationList=projectconfig, compatibilityVersion="Xcode 14.0", developmentRegion="en", hasScannedForEncodings=0, knownRegions=["en", "Base"], mainGroup=main, productRefGroup=products, projectDirPath="", projectRoot="", targets=[target])
with (project / "project.pbxproj").open("wb") as f:
    plistlib.dump({"archiveVersion": "1", "classes": {}, "objectVersion": "56", "objects": objects, "rootObject": rootobj}, f)
schemes = project / "xcshareddata/xcschemes"
schemes.mkdir(parents=True, exist_ok=True)
ref = '<BuildableReference BuildableIdentifier="primary" BlueprintIdentifier="%s" BuildableName="Torrbaby.app" BlueprintName="Torrbaby" ReferencedContainer="container:Torrbaby.xcodeproj"/>' % target
(schemes / "Torrbaby.xcscheme").write_text('''<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion="2700" version="1.3">
<BuildAction parallelizeBuildables="YES" buildImplicitDependencies="YES"><BuildActionEntries><BuildActionEntry buildForTesting="YES" buildForRunning="YES" buildForProfiling="YES" buildForArchiving="YES" buildForAnalyzing="YES">''' + ref + '''</BuildActionEntry></BuildActionEntries></BuildAction>
<TestAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB"/>
<LaunchAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB" launchStyle="0" useCustomWorkingDirectory="NO" ignoresPersistentStateOnLaunch="NO" debugDocumentVersioning="YES" debugServiceExtension="internal" allowLocationSimulation="YES"><BuildableProductRunnable runnableDebuggingMode="0">''' + ref + '''</BuildableProductRunnable></LaunchAction>
<ProfileAction buildConfiguration="Release" shouldUseLaunchSchemeArgsEnv="YES" savedToolIdentifier="" useCustomWorkingDirectory="NO" debugDocumentVersioning="YES"><BuildableProductRunnable runnableDebuggingMode="0">''' + ref + '''</BuildableProductRunnable></ProfileAction>
<AnalyzeAction buildConfiguration="Debug"/><ArchiveAction buildConfiguration="Release" revealArchiveInOrganizer="YES"/>
</Scheme>
''')
info = {"CFBundleURLTypes": [{"CFBundleURLName": "org.torrbaby.magnet", "CFBundleURLSchemes": ["magnet"], "CFBundleTypeRole": "Viewer"}], "CFBundleDisplayName": "Torrbaby", "CFBundleName": "$(PRODUCT_NAME)", "CFBundleIdentifier": "$(PRODUCT_BUNDLE_IDENTIFIER)", "CFBundleExecutable": "$(EXECUTABLE_NAME)", "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "$(MARKETING_VERSION)", "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)", "LSApplicationCategoryType": "public.app-category.utilities", "NSAppTransportSecurity": {"NSAllowsArbitraryLoads": True}, "NSLocalNetworkUsageDescription": "Connect to your Torrbaby server and stream your own media.", "UILaunchScreen": {}, "UIApplicationSceneManifest": {"UIApplicationSupportsMultipleScenes": True}, "UISupportedInterfaceOrientations": ["UIInterfaceOrientationPortrait", "UIInterfaceOrientationLandscapeLeft", "UIInterfaceOrientationLandscapeRight"], "NSHumanReadableCopyright": "Copyright © 2026 Jon Thomason. MIT License."}
with (root / "Torrbaby/Info.plist").open("wb") as f:
    plistlib.dump(info, f)
