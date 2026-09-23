import { Platform, useColorScheme } from 'react-native';

export function useTheme() {
  const dark = useColorScheme() === 'dark';
  const colors = dark ? {
    background: '#0B1220', surface: '#121B2D', elevated: '#18243A', ink: '#F4F7FB', muted: '#A6B2C5', line: '#2A3850', accent: '#69A8FF', accentInk: '#061326', success: '#5FD39A', warning: '#F5BE64', danger: '#FF7E86',
  } : {
    background: '#F5F7FA', surface: '#FFFFFF', elevated: '#EEF3F9', ink: '#152033', muted: '#607087', line: '#DCE3EC', accent: '#1668D9', accentInk: '#FFFFFF', success: '#138A58', warning: '#A86400', danger: '#B62838',
  };
  return { dark, colors, statusBar: dark ? 'light' as const : 'dark' as const, platformPadding: Platform.OS === 'ios' ? 20 : 16 };
}
