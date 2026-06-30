import React from "react";
import { Link } from "wouter";
import { useGetProposals } from "@workspace/api-client-react";
import { timeAgo } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import { Gavel } from "lucide-react";

export default function Governance() {
  const { data: proposals, isLoading } = useGetProposals();

  const getStatusColor = (status: string) => {
    switch (status.toLowerCase()) {
      case 'active': return 'default';
      case 'passed': return 'outline'; // typically custom green, outline is fine
      case 'rejected': return 'destructive';
      case 'pending': return 'secondary';
      default: return 'secondary';
    }
  };

  const calculatePercentage = (yes: string, no: string, abstain: string) => {
    const y = parseFloat(yes) || 0;
    const n = parseFloat(no) || 0;
    const a = parseFloat(abstain) || 0;
    const total = y + n + a;
    if (total === 0) return { yes: 0, no: 0, abstain: 0 };
    return {
      yes: (y / total) * 100,
      no: (n / total) * 100,
      abstain: (a / total) * 100
    };
  };

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
          <Gavel className="text-primary w-6 h-6" /> Governance (ZEPs)
        </h2>
        <p className="text-muted-foreground">Zebvix Evolution Proposals and on-chain voting.</p>
      </div>

      <Card>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-4 space-y-4">
              {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-24 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-16">ID</TableHead>
                  <TableHead>Proposal</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="w-[300px]">Voting Results</TableHead>
                  <TableHead className="text-right">End Time</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {proposals?.map((proposal) => {
                  const pct = calculatePercentage(proposal.yesVotes, proposal.noVotes, proposal.abstainVotes);
                  return (
                    <TableRow key={proposal.id}>
                      <TableCell className="font-mono text-sm font-bold text-muted-foreground">
                        {proposal.zepNumber ? `ZEP-${proposal.zepNumber}` : `#${proposal.id}`}
                      </TableCell>
                      <TableCell>
                        <div className="flex flex-col gap-1">
                          <Link href={`/governance/${proposal.id}`} className="font-bold text-base hover:text-primary transition-colors">
                            {proposal.title}
                          </Link>
                          <Badge variant="outline" className="w-fit text-[10px] uppercase bg-muted/50">{proposal.type}</Badge>
                        </div>
                      </TableCell>
                      <TableCell>
                        <Badge variant={getStatusColor(proposal.status)} className="uppercase text-[10px]">
                          {proposal.status}
                        </Badge>
                      </TableCell>
                      <TableCell>
                        <div className="space-y-1.5 w-full">
                          <div className="flex w-full h-2 bg-muted rounded-full overflow-hidden">
                            <div style={{ width: `${pct.yes}%` }} className="bg-green-500 h-full" />
                            <div style={{ width: `${pct.no}%` }} className="bg-destructive h-full" />
                            <div style={{ width: `${pct.abstain}%` }} className="bg-yellow-500 h-full" />
                          </div>
                          <div className="flex justify-between text-[10px] font-mono text-muted-foreground">
                            <span className="text-green-500">Y: {pct.yes.toFixed(1)}%</span>
                            <span className="text-destructive">N: {pct.no.toFixed(1)}%</span>
                          </div>
                        </div>
                      </TableCell>
                      <TableCell className="text-right text-xs">
                        <div className="flex flex-col items-end">
                          <span className="font-medium">{new Date(proposal.endTime).toLocaleDateString()}</span>
                          <span className="text-muted-foreground">{timeAgo(proposal.endTime)}</span>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                })}
                {proposals?.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={5} className="text-center py-8 text-muted-foreground">
                      No proposals found.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
