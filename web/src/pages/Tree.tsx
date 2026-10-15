import { useParams } from "react-router-dom";
import { TreeBrowser } from "../components/TreeBrowser";
import { useRepo } from "../components/RepoShell";

export default function Tree() {
  const { name } = useRepo();
  const { rev = "main", "*": splat = "" } = useParams();
  const path = splat.replace(/\/+$/, "");
  return <TreeBrowser repo={name} rev={decodeURIComponent(rev)} path={path} />;
}
