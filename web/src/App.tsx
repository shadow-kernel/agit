import { Suspense, lazy, useEffect, useState } from "react";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { initApi } from "./api/client";
import { Layout } from "./components/Layout";
import { RepoShell } from "./components/RepoShell";
import { Skeleton } from "./components/ui";

const RepoList = lazy(() => import("./pages/RepoList"));
const RepoHome = lazy(() => import("./pages/RepoHome"));
const Tree = lazy(() => import("./pages/Tree"));
const Blob = lazy(() => import("./pages/Blob"));
const Commits = lazy(() => import("./pages/Commits"));
const CommitDetail = lazy(() => import("./pages/CommitDetail"));
const Pulls = lazy(() => import("./pages/Pulls"));
const PullNew = lazy(() => import("./pages/PullNew"));
const PullDetail = lazy(() => import("./pages/PullDetail"));
const Agents = lazy(() => import("./pages/Agents"));
const Leases = lazy(() => import("./pages/Leases"));
const Knowledge = lazy(() => import("./pages/Knowledge"));
const KnowledgeNote = lazy(() => import("./pages/KnowledgeNote"));
const NotFound = lazy(() => import("./pages/NotFound"));

const fallback = <div className="box" aria-busy><Skeleton lines={6} /></div>;
const pageFallback = <main className="container">{fallback}</main>;

export default function App() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    initApi().finally(() => setReady(true));
  }, []);
  if (!ready) {
    return (
      <>
        <header className="header"><span className="logo">agit</span></header>
        {pageFallback}
      </>
    );
  }
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Suspense fallback={pageFallback}><RepoList /></Suspense>} />
          <Route path="/r/:repo" element={<RepoShell />}>
            <Route index element={<Suspense fallback={fallback}><RepoHome /></Suspense>} />
            <Route path="tree/:rev/*" element={<Suspense fallback={fallback}><Tree /></Suspense>} />
            <Route path="tree/:rev" element={<Suspense fallback={fallback}><Tree /></Suspense>} />
            <Route path="blob/:rev/*" element={<Suspense fallback={fallback}><Blob /></Suspense>} />
            <Route path="commits" element={<Suspense fallback={fallback}><Commits /></Suspense>} />
            <Route path="commit/:id" element={<Suspense fallback={fallback}><CommitDetail /></Suspense>} />
            <Route path="pulls" element={<Suspense fallback={fallback}><Pulls /></Suspense>} />
            <Route path="pulls/new" element={<Suspense fallback={fallback}><PullNew /></Suspense>} />
            <Route path="pulls/:n/*" element={<Suspense fallback={fallback}><PullDetail /></Suspense>} />
            <Route path="agents" element={<Suspense fallback={fallback}><Agents /></Suspense>} />
            <Route path="leases" element={<Suspense fallback={fallback}><Leases /></Suspense>} />
            <Route path="knowledge" element={<Suspense fallback={fallback}><Knowledge /></Suspense>} />
            <Route path="knowledge/:id" element={<Suspense fallback={fallback}><KnowledgeNote /></Suspense>} />
            <Route path="*" element={<Navigate to="." replace />} />
          </Route>
          <Route path="*" element={<Suspense fallback={pageFallback}><NotFound /></Suspense>} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
