import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./app.css";
import tokens from "./design-tokens.json";

startApp(App, { tokens });
