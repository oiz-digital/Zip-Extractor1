import React from "react";
import { Link, useParams } from "wouter";
import { useGetProposal } from "@workspace/api-client-react";
import { formatNumber, formatAddress, timeAgo } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Badge } from "@/components/ui/badge";
import { ArrowLeft, Gavel, FileText } from "lucide-react";

export default function ProposalDetail() {
  const { id } = useParams<{ id: string }>();
  const { data: proposal, isLoading } = useGetProposal(Number(id), { 
    query: { enabled: !!id } 
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  if (!proposal) {
    return <div>Proposal not found</div>;
  }

  const y = parseFloat(proposal.yesVotes) || 0;
  const n = parseFloat(proposal.noVotes) || 0;
  const a = parseFloat(proposal.abstainVotes) || 0;
  const total = y + n + a;
  const yesPct = total > 0 ? (y / total) * 100 : 0;
  const noPct = total > 0 ? (n / total) * 100 : 0;
  const abstainPct = total > 0 ? (a / total) * 100 : 0;

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4 mb-4">
        <Link href="/governance" className="p-2 hover:bg-accent rounded-full transition-colors">
          <ArrowLeft className="w-5 h-5" />
        </Link>
        <div>
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
              <span className="text-muted-foreground">
                {proposal.zepNumber ? `ZEP-${proposal.zepNumber}` : `#${proposal.id}`}
              </span>
              {proposal.title}
            </h2>
            <Badge variant={proposal.status === 'active' ? 'default' : proposal.status === 'rejected' ? 'destructive' : 'secondary'} className="uppercase">
              {proposal.status}
            </Badge>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        <div className="lg:col-span-2 space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Description</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="prose prose-sm dark:prose-invert max-w-none whitespace-pre-wrap">
                {proposal.description}
              </div>
            </CardContent>
          </Card>

          {proposal.changes && proposal.changes.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle>Proposed Changes</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="space-y-2">
                  {proposal.changes.map((change, i) => (
                    <div key={i} className="bg-muted p-3 rounded-md font-mono text-xs border border-border">
                      {change}
                    </div>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}
        </div>

        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Information</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-muted-foreground text-sm">Proposer</span>
                <Link href={`/address/${proposal.proposer}`} className="text-sm font-mono text-primary hover:underline">
                  {formatAddress(proposal.proposer, 8)}
                </Link>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-muted-foreground text-sm">Type</span>
                <span className="text-sm uppercase">{proposal.type}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-muted-foreground text-sm">End Time</span>
                <span className="text-sm font-medium text-right">
                  {new Date(proposal.endTime).toLocaleString()}<br/>
                  <span className="text-xs text-muted-foreground">{timeAgo(proposal.endTime)}</span>
                </span>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Voting Results</CardTitle>
              <CardDescription>Total votes: {formatNumber(total, 0)} ZBX</CardDescription>
            </CardHeader>
            <CardContent className="space-y-6">
              <div className="space-y-2">
                <div className="flex justify-between text-sm">
                  <span className="font-bold text-green-500">Yes</span>
                  <span className="font-mono">{formatNumber(y, 0)} ({yesPct.toFixed(2)}%)</span>
                </div>
                <div className="w-full h-3 bg-muted rounded-full overflow-hidden">
                  <div style={{ width: `${yesPct}%` }} className="bg-green-500 h-full" />
                </div>
              </div>

              <div className="space-y-2">
                <div className="flex justify-between text-sm">
                  <span className="font-bold text-destructive">No</span>
                  <span className="font-mono">{formatNumber(n, 0)} ({noPct.toFixed(2)}%)</span>
                </div>
                <div className="w-full h-3 bg-muted rounded-full overflow-hidden">
                  <div style={{ width: `${noPct}%` }} className="bg-destructive h-full" />
                </div>
              </div>

              <div className="space-y-2">
                <div className="flex justify-between text-sm">
                  <span className="font-bold text-yellow-500">Abstain</span>
                  <span className="font-mono">{formatNumber(a, 0)} ({abstainPct.toFixed(2)}%)</span>
                </div>
                <div className="w-full h-3 bg-muted rounded-full overflow-hidden">
                  <div style={{ width: `${abstainPct}%` }} className="bg-yellow-500 h-full" />
                </div>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
