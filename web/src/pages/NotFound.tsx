import { Link } from "react-router-dom";

export default function NotFound() {
  return (
    <main className="container">
      <div className="notfound">
        <h1>404</h1>
        <p className="muted">This page does not exist.</p>
        <Link className="btn" to="/">Back to repositories</Link>
      </div>
    </main>
  );
}
