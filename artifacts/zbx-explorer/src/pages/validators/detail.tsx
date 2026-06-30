import React from "react";
import { Link, useParams } from "wouter";
import { useGetValidator } from "@workspace/api-client-react";
import { formatNumber, formatAddress } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Badge } from "@/components/ui/badge";
import { Users, ArrowLeft, Globe, Info } from "lucide-react";

export default function ValidatorDetail() {
  const { address } = useParams<{ address: string }>();
  const { data: validator, isLoading } = useGetValidator(address as string, { 
    query: { enabled: !!address } 
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  if (!validator) {
    return <div>Validator not found</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4 mb-4">
        <Link href="/validators" className="p-2 hover:bg-accent rounded-full transition-colors">
          <ArrowLeft className="w-5 h-5" />
        </Link>
        <div>
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
              <Users className="w-6 h-6 text-primary" />
              {validator.moniker || 'Unknown Validator'}
            </h2>
            <Badge variant={validator.status === 'active' ? 'default' : 'secondary'} className="uppercase">
              {validator.status}
            </Badge>
          </div>
          <p className="text-muted-foreground font-mono text-xs">{validator.address}</p>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Identity</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {validator.description && (
              <div className="flex items-start gap-2 py-2 border-b border-border/50">
                <Info className="w-4 h-4 mt-0.5 text-muted-foreground" />
                <span className="text-sm">{validator.description}</span>
              </div>
            )}
            {validator.website && (
              <div className="flex items-center gap-2 py-2 border-b border-border/50">
                <Globe className="w-4 h-4 text-muted-foreground" />
                <a href={validator.website} target="_blank" rel="noreferrer" className="text-sm text-primary hover:underline">
                  {validator.website}
                </a>
              </div>
            )}
            <div className="grid grid-cols-2 gap-4 pt-2">
              <div>
                <span className="text-muted-foreground text-xs uppercase block mb-1">Rank</span>
                <span className="text-xl font-bold font-mono">#{validator.rank}</span>
              </div>
              <div>
                <span className="text-muted-foreground text-xs uppercase block mb-1">Delegators</span>
                <span className="text-xl font-bold font-mono">{formatNumber(validator.delegators, 0)}</span>
              </div>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Metrics</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Voting Power</span>
              <span className="text-sm font-mono font-medium">{formatNumber(validator.votingPower, 0)}</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Total Stake</span>
              <span className="text-sm font-mono font-medium">{formatNumber(validator.totalStake, 0)} ZBX</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Self Stake</span>
              <span className="text-sm font-mono font-medium">{formatNumber(validator.selfStake, 0)} ZBX</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Commission</span>
              <span className="text-sm font-mono font-medium">{formatNumber(validator.commission * 100, 2)}%</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Uptime (last 10k blocks)</span>
              <span className={`text-sm font-mono font-medium ${validator.uptime > 99 ? 'text-green-500' : 'text-yellow-500'}`}>
                {formatNumber(validator.uptime, 2)}%
              </span>
            </div>
          </CardContent>
        </Card>
      </div>

      {validator.recentBlocks && validator.recentBlocks.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Recent Blocks Proposed</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-wrap gap-2">
              {validator.recentBlocks.map(blockNum => (
                <Link key={blockNum} href={`/blocks/${blockNum}`}>
                  <Badge variant="outline" className="font-mono hover:bg-primary/10 transition-colors cursor-pointer">
                    {blockNum}
                  </Badge>
                </Link>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
