import { createTheme } from "@mui/material/styles";

export const theme = createTheme({
  cssVariables: { colorSchemeSelector: "class" },
  colorSchemes: {
    light: {
      palette: {
        primary: { main: "#6554c0" },
        background: { default: "#f5f5fa", paper: "#ffffff" },
        success: { main: "#24734e" },
      },
    },
    dark: {
      palette: {
        primary: { main: "#c4b5fd" },
        background: { default: "#121218", paper: "#1c1b23" },
        success: { main: "#81d6aa" },
      },
    },
  },
  shape: { borderRadius: 14 },
  typography: {
    fontFamily: '"Noto Sans SC", "Microsoft YaHei", system-ui, sans-serif',
    h4: { fontWeight: 700, fontSize: "1.7rem", letterSpacing: "-0.04em" },
    h6: { fontWeight: 650, fontSize: "1rem" },
    button: { textTransform: "none", fontWeight: 600 },
    body2: { lineHeight: 1.7 },
  },
  components: {
    MuiButton: { defaultProps: { disableElevation: true, size: "small" } },
    MuiTextField: { defaultProps: { size: "small", fullWidth: true } },
    MuiPaper: { defaultProps: { elevation: 0 } },
    MuiCard: { defaultProps: { variant: "outlined" } },
    MuiTooltip: { defaultProps: { arrow: true } },
    MuiCssBaseline: {
      styleOverrides: {
        body: { minWidth: 360 },
        "*": { boxSizing: "border-box" },
      },
    },
  },
});
