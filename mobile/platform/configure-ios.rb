#!/usr/bin/env ruby
# Reproducibly adds the Packet Tunnel target after Expo prebuild.
require 'xcodeproj'

project_path = ARGV.fetch(0) { abort 'usage: configure-ios.rb path/to/MyProxy.xcodeproj' }
project = Xcodeproj::Project.open(project_path)
app = project.targets.find { |t| t.product_type == 'com.apple.product-type.application' } or abort 'application target not found'
extension = project.targets.find { |t| t.name == 'MyProxyPacketTunnel' }
unless extension
  extension = project.new_target(:app_extension, 'MyProxyPacketTunnel', :ios, '15.0')
  extension.product_type = 'com.apple.product-type.app-extension.network-extension'
  extension.build_configurations.each do |config|
    config.build_settings['PRODUCT_BUNDLE_IDENTIFIER'] = 'one.leaper.myproxy.xray.PacketTunnel'
    config.build_settings['INFOPLIST_FILE'] = 'mobile/ios-extension/Info.plist'
    config.build_settings['CODE_SIGN_ENTITLEMENTS'] = 'mobile/ios-extension/MyProxyPacketTunnel.entitlements'
    config.build_settings['SWIFT_VERSION'] = '5.9'
    config.build_settings['FRAMEWORK_SEARCH_PATHS'] = '$(inherited) $(PROJECT_DIR)/mobile/app/modules/myproxy/ios/Frameworks'
  end
  group = project.main_group.new_group('MyProxyPacketTunnel')
  paths = Dir['mobile/ios-extension/Sources/*.swift'] + ['mobile/ios-extension/Info.plist', 'mobile/ios-extension/MyProxyPacketTunnel.entitlements']
  paths.each { |path| group << project.new_file(path) }
  group.files.each { |file| extension.add_file_references([file]) }
  %w[MyProxyCore.xcframework MyProxyNetwork.xcframework].each do |name|
    ref = project.new_file("mobile/app/modules/myproxy/ios/Frameworks/#{name}")
    extension.frameworks_build_phase.add_file_reference(ref)
  end
  phase = app.new_copy_files_build_phase('Embed Packet Tunnel')
  phase.dst_subfolder_spec = :plugins
  phase.add_file_reference(extension.product_reference)
end
project.save
