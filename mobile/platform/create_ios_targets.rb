#!/usr/bin/env ruby
# Reproducible target generator invoked after Expo prebuild. It deliberately
# edits only the generated iOS project and never changes the mobile UI source.
require 'xcodeproj'

project_path = ARGV.fetch(0) { abort 'usage: create_ios_targets.rb path/to/MyProxy.xcodeproj' }
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
  end
  group = project.main_group.new_group('MyProxyPacketTunnel')
  Dir['mobile/ios-extension/Sources/*.swift'].each { |path| group << project.new_file(path) }
  group << project.new_file('mobile/ios-extension/Info.plist')
  group << project.new_file('mobile/ios-extension/MyProxyPacketTunnel.entitlements')
  group.files.each { |file| extension.add_file_references([file]) }
  phase = app.new_copy_files_build_phase('Embed Packet Tunnel')
  phase.dst_subfolder_spec = :plugins
  phase.add_file_reference(extension.product_reference)
end
project.save
