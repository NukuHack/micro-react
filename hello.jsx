import { Helloxx as Hello } from "./inner.jsx";

function Lol({ name }) {
  return <div class="hello-jsx">👋 Hello, {name}! (rendered from a real .jsx file)</div>;
}

export default function HelloPage() {
  return (<div style='position:relative;top:1rem;left:1rem;z-index:999'>
    <Hello name="micro-react" />
    <Lol name="Looool" />
  </div>);
}


